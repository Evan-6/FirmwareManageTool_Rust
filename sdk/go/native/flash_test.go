package gdbnative

import (
	"bytes"
	"encoding/binary"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

// --- fixtures ---------------------------------------------------------------

// elfSegment is one loadable segment of a synthetic ELF.
type elfSegment struct {
	address uint32
	payload []byte
}

// syntheticELF builds a minimal ELF32 little-endian file with the given
// segments, laid out the way the Rust unit test does.
func syntheticELF(segments []elfSegment) []byte {
	const programHeader = 52
	headerSize := programHeader + len(segments)*32
	out := make([]byte, headerSize)
	copy(out, []byte{0x7f, 'E', 'L', 'F'})
	out[4] = 1
	out[5] = 1
	binary.LittleEndian.PutUint32(out[28:], programHeader)
	binary.LittleEndian.PutUint16(out[42:], 32)
	binary.LittleEndian.PutUint16(out[44:], uint16(len(segments)))
	for index, segment := range segments {
		base := programHeader + index*32
		binary.LittleEndian.PutUint32(out[base:], 1) // PT_LOAD
		binary.LittleEndian.PutUint32(out[base+4:], uint32(len(out)))
		binary.LittleEndian.PutUint32(out[base+12:], segment.address)
		binary.LittleEndian.PutUint32(out[base+16:], uint32(len(segment.payload)))
		out = append(out, segment.payload...)
	}
	return out
}

// flashELF is one 256-byte flash segment at the XIP base: exactly one block.
func flashELF() []byte {
	return syntheticELF([]elfSegment{{address: XIPBase, payload: bytes.Repeat([]byte{0x5A}, 256)}})
}

func blockField(uf2 []byte, block, index int) uint32 {
	at := block*UF2BlockSize + index*4
	return binary.LittleEndian.Uint32(uf2[at:])
}

func exeName(name string) string {
	if runtime.GOOS == "windows" {
		return name + ".exe"
	}
	return name
}

// fakeRunner records the command lines it was asked to run and can fail one of
// them, which is how the build sequence is tested without cargo.
type fakeRunner struct {
	mu     sync.Mutex
	failOn string
	calls  [][]string
}

func (f *fakeRunner) Run(program string, args []string, _ time.Duration, _ Cancel, log func(Stream, string)) (RunResult, error) {
	joined := strings.Join(append([]string{program}, args...), " ")
	f.mu.Lock()
	f.calls = append(f.calls, append([]string{program}, args...))
	f.mu.Unlock()
	if log != nil {
		log(Stdout, "fake "+filepath.Base(program))
	}
	if f.failOn != "" && strings.Contains(joined, f.failOn) {
		exit := 9
		return RunResult{Exit: &exit, Stderr: "injected failure"}, nil
	}
	exit := 0
	return RunResult{Exit: &exit, Stdout: "ok"}, nil
}

func (f *fakeRunner) commands() [][]string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([][]string(nil), f.calls...)
}

// writeFirmware lays out <root>/firmware/app with a Cargo.toml and, when
// elfBytes is not nil, the release artifact at the path cargo would leave it.
func writeFirmware(t *testing.T, elfBytes []byte) FlashConfig {
	t.Helper()
	root := t.TempDir()
	cfg := FlashConfig{ManifestDir: filepath.Join(root, "firmware")}
	if err := os.MkdirAll(cfg.AppDir(), 0o755); err != nil {
		t.Fatalf("mkdir app: %v", err)
	}
	if err := os.WriteFile(cfg.Manifest(), []byte("[package]\nname = \"xiao-firmware\"\n"), 0o644); err != nil {
		t.Fatalf("write manifest: %v", err)
	}
	if elfBytes != nil {
		artifact := cfg.ELF()
		if err := os.MkdirAll(filepath.Dir(artifact), 0o755); err != nil {
			t.Fatalf("mkdir target: %v", err)
		}
		if err := os.WriteFile(artifact, elfBytes, 0o644); err != nil {
			t.Fatalf("write elf: %v", err)
		}
	}
	return cfg
}

// fakeCargo drops a cargo binary, plus rustup next to it, into the temp tree.
func fakeCargo(t *testing.T, withRustup bool) string {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "bin")
	if err := os.MkdirAll(bin, 0o755); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}
	cargo := filepath.Join(bin, exeName("cargo"))
	if err := os.WriteFile(cargo, []byte("fake"), 0o755); err != nil {
		t.Fatalf("write cargo: %v", err)
	}
	if withRustup {
		rustup := filepath.Join(bin, exeName("rustup"))
		if err := os.WriteFile(rustup, []byte("fake"), 0o755); err != nil {
			t.Fatalf("write rustup: %v", err)
		}
	}
	return cargo
}

// recordingProgress keeps every line and stage so a test can assert on the
// order the user sees.
type recordingProgress struct {
	mu       sync.Mutex
	stages   []uint8
	messages []string
	logs     []string
	drives   []string
}

func (p *recordingProgress) Log(line string) {
	p.mu.Lock()
	p.logs = append(p.logs, line)
	p.mu.Unlock()
}

func (p *recordingProgress) Stage(stage uint8, message string) {
	p.mu.Lock()
	p.stages = append(p.stages, stage)
	p.messages = append(p.messages, message)
	p.mu.Unlock()
}

func (p *recordingProgress) ChooseDrive(candidates []string) (string, error) {
	p.mu.Lock()
	p.drives = append(p.drives, candidates...)
	p.mu.Unlock()
	return ProgressFuncs{}.ChooseDrive(candidates)
}

func (p *recordingProgress) sawLog(fragment string) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	for _, line := range p.logs {
		if strings.Contains(line, fragment) {
			return true
		}
	}
	return false
}

func (p *recordingProgress) stageList() []uint8 {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]uint8(nil), p.stages...)
}

// flashSeams replaces the three outside-world seams and restores them.
func flashSeams(t *testing.T, vol func() ([]string, error), enum func() ([]Entry, error), verify func(Entry, uint8) (Verified, error)) {
	t.Helper()
	previousVolumes, previousEnum, previousVerify := listVolumes, enumerateDevices, openForVerify
	previousWait, previousHandshake, previousPoll := driveWait, handshakeWait, pollInterval
	listVolumes, enumerateDevices, openForVerify = vol, enum, verify
	t.Cleanup(func() {
		listVolumes, enumerateDevices, openForVerify = previousVolumes, previousEnum, previousVerify
		driveWait, handshakeWait, pollInterval = previousWait, previousHandshake, previousPoll
	})
}

// --- UF2 --------------------------------------------------------------------

func localAppData() string {
	if value := os.Getenv("LOCALAPPDATA"); value != "" {
		return value
	}
	return filepath.Join(os.Getenv("USERPROFILE"), "AppData", "Local")
}

// referenceImage is one UF2 file that was actually flashed from this repository.
type referenceImage struct {
	path string
	data []byte
}

// referenceImages lists the UF2 files a real build left behind: the copy the
// tool keeps and the one cargo's target directory holds.
func referenceImages(t *testing.T) []referenceImage {
	t.Helper()
	candidates := []string{
		filepath.Join(localAppData(), "FirmwareManageTool_Rust", "Firmware.uf2"),
		filepath.Join("..", "..", "..", "firmware", "app", "target", TargetTriple, "release", AppCrate+".uf2"),
	}
	references := make([]referenceImage, 0, len(candidates))
	for _, path := range candidates {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		references = append(references, referenceImage{path: path, data: data})
	}
	return references
}

// checkBlockLayout asserts the structural facts of a packed image that no
// recompilation changes: 512-byte blocks, sequential 256-byte addresses from
// the XIP base, the RP2040 family id and a matching block count.
func checkBlockLayout(t *testing.T, packed []byte) {
	t.Helper()
	if len(packed)%UF2BlockSize != 0 {
		t.Fatalf("image is %d bytes, not a whole number of %d-byte blocks", len(packed), UF2BlockSize)
	}
	blocks := UF2Blocks(packed)
	for index := 0; index < blocks; index++ {
		if field := blockField(packed, index, 2); field != 0x00002000 {
			t.Fatalf("block %d flags = %#x, want the family flag", index, field)
		}
		if field := blockField(packed, index, 3); field != XIPBase+uint32(index)*UF2Payload {
			t.Fatalf("block %d address = %#x, want %#x", index, field, XIPBase+uint32(index)*UF2Payload)
		}
		if field := blockField(packed, index, 4); field != UF2Payload {
			t.Fatalf("block %d payload size = %d", index, field)
		}
		if field := blockField(packed, index, 5); field != uint32(index) {
			t.Fatalf("block %d number = %d", index, field)
		}
		if field := blockField(packed, index, 6); field != uint32(blocks) {
			t.Fatalf("block %d count = %d, want %d", index, field, blocks)
		}
		if field := blockField(packed, index, 7); field != FamilyRP2040 {
			t.Fatalf("block %d family = %#x", index, field)
		}
	}
}

// TestUFPackMatchesTheFlashedArtifact packs the ELF this repository built and
// compares it byte for byte with a UF2 the bootloader actually accepted, which
// is the image the tool left behind on hardware.
func TestUFPackMatchesTheFlashedArtifact(t *testing.T) {
	elfPath := filepath.Join("..", "..", "..", "firmware", "app", "target", TargetTriple, "release", AppCrate)
	elf, err := os.ReadFile(elfPath)
	if err != nil {
		t.Skipf("no firmware ELF at %s (%v); build it with cargo build --release --target %s --features %s",
			elfPath, err, TargetTriple, DefaultFeatures)
	}
	references := referenceImages(t)
	if len(references) == 0 {
		t.Skipf("no reference UF2 beside %s; flash once with the tool to produce %s",
			elfPath, filepath.Join(localAppData(), "FirmwareManageTool_Rust", "Firmware.uf2"))
	}
	primary := references[0]
	if len(primary.data) != 65536 || UF2Blocks(primary.data) != 128 {
		t.Fatalf("reference %s is %d bytes in %d blocks, want 65536 in 128",
			primary.path, len(primary.data), UF2Blocks(primary.data))
	}
	packed, err := UFPack(elf)
	if err != nil {
		t.Fatalf("pack: %v", err)
	}
	checkBlockLayout(t, packed)
	for _, reference := range references {
		if bytes.Equal(packed, reference.data) {
			t.Logf("matches %s byte for byte (%d bytes, %d blocks)", reference.path, len(packed), UF2Blocks(packed))
			return
		}
	}
	// A rebuild after the reference was written makes the comparison
	// meaningless — the image on disk belongs to an older source revision —
	// so say so instead of reporting the packer as broken.
	elfInfo, statErr := os.Stat(elfPath)
	newest := time.Time{}
	for _, reference := range references {
		info, err := os.Stat(reference.path)
		if err != nil {
			continue
		}
		if info.ModTime().After(newest) {
			newest = info.ModTime()
		}
	}
	if statErr == nil && elfInfo.ModTime().After(newest) {
		t.Skipf("the ELF was rebuilt (%s) after every reference UF2 (%s): the block layout of %d bytes in %d blocks was checked, but the bytes belong to different builds; flash once to refresh the reference",
			elfInfo.ModTime().Format(time.RFC3339), newest.Format(time.RFC3339), len(packed), UF2Blocks(packed))
	}
	want := primary.data
	if len(packed) != len(want) {
		t.Fatalf("packed %d bytes in %d blocks, reference %s is %d bytes in %d blocks",
			len(packed), UF2Blocks(packed), primary.path, len(want), UF2Blocks(want))
	}
	for index := range packed {
		if packed[index] != want[index] {
			t.Fatalf("first difference at byte %d (block %d, offset %d): got %#02x want %#02x",
				index, index/UF2BlockSize, index%UF2BlockSize, packed[index], want[index])
		}
	}
}

func TestUFPackAlignsAndOrdersBlocksByAddress(t *testing.T) {
	// boot2 is last in file order but lowest in address, plus an unaligned
	// segment: both must come out sorted and 256-byte aligned.
	image := syntheticELF([]elfSegment{
		{address: 0x10000100, payload: []byte{1, 2, 3, 4}},
		{address: 0x10000000, payload: bytes.Repeat([]byte{0xAA}, 256)},
	})
	packed, err := UFPack(image)
	if err != nil {
		t.Fatalf("pack: %v", err)
	}
	if UF2Blocks(packed) != 2 {
		t.Fatalf("blocks = %d, want 2", UF2Blocks(packed))
	}
	if field := blockField(packed, 0, 3); field != 0x10000000 {
		t.Errorf("block 0 address = %#x", field)
	}
	if field := blockField(packed, 1, 3); field != 0x10000100 {
		t.Errorf("block 1 address = %#x", field)
	}
	for index := 0; index < 2; index++ {
		if field := blockField(packed, index, 0); field != 0x0A324655 {
			t.Errorf("block %d magic start 0 = %#x", index, field)
		}
		if field := blockField(packed, index, 1); field != 0x9E5D5157 {
			t.Errorf("block %d magic start 1 = %#x", index, field)
		}
		if field := blockField(packed, index, 2); field != 0x00002000 {
			t.Errorf("block %d flags = %#x", index, field)
		}
		if field := blockField(packed, index, 4); field != 256 {
			t.Errorf("block %d payload size = %d", index, field)
		}
		if field := blockField(packed, index, 5); field != uint32(index) {
			t.Errorf("block %d number = %d", index, field)
		}
		if field := blockField(packed, index, 6); field != 2 {
			t.Errorf("block %d count = %d", index, field)
		}
		if field := blockField(packed, index, 7); field != FamilyRP2040 {
			t.Errorf("block %d family = %#x", index, field)
		}
		end := index*UF2BlockSize + UF2BlockSize - 4
		if field := binary.LittleEndian.Uint32(packed[end:]); field != 0x0AB16F30 {
			t.Errorf("block %d magic end = %#x", index, field)
		}
	}
	if !bytes.Equal(packed[32:288], bytes.Repeat([]byte{0xAA}, 256)) {
		t.Error("block 0 does not carry boot2")
	}
	if !bytes.Equal(packed[UF2BlockSize+32:UF2BlockSize+36], []byte{1, 2, 3, 4}) {
		t.Error("block 1 does not carry the unaligned segment")
	}
	if !bytes.Equal(packed[UF2BlockSize+36:UF2BlockSize+288], bytes.Repeat([]byte{0xFF}, 252)) {
		t.Error("the gap after the unaligned segment is not erased (0xFF)")
	}
}

func TestUFPackRejectsAnImageWithoutFlashSegments(t *testing.T) {
	for name, image := range map[string][]byte{
		"no flash segment": syntheticELF([]elfSegment{{address: 0x20000000, payload: []byte{1, 2, 3, 4}}}),
		"not an elf":       []byte("not an elf at all, just text of some length"),
	} {
		if _, err := UFPack(image); err == nil {
			t.Errorf("%s: UFPack accepted it", name)
		} else if CodeOf(err) != Invalid {
			t.Errorf("%s: code = %v, want INVALID", name, CodeOf(err))
		}
	}
}

// --- build ------------------------------------------------------------------

func TestBuildRunsThePinnedCargoCommandLine(t *testing.T) {
	cfg := writeFirmware(t, flashELF())
	cfg.CargoPath = fakeCargo(t, true)
	runner := &fakeRunner{}
	progress := &recordingProgress{}

	image, err := Build(cfg, runner, NeverCancel, progress)
	if err != nil {
		t.Fatalf("build: %v", err)
	}
	if image.Blocks != 1 || len(image.Bytes) != UF2BlockSize {
		t.Fatalf("image = %d bytes / %d blocks, want one block", len(image.Bytes), image.Blocks)
	}
	if image.ELF != cfg.ELF() {
		t.Errorf("image ELF = %q, want %q", image.ELF, cfg.ELF())
	}
	calls := runner.commands()
	if len(calls) != 2 {
		t.Fatalf("calls = %v, want rustup + cargo", calls)
	}
	if !strings.HasSuffix(calls[0][0], exeName("rustup")) || strings.Join(calls[0][1:], " ") != "target add "+TargetTriple {
		t.Errorf("first call = %v, want rustup target add %s", calls[0], TargetTriple)
	}
	build := calls[1]
	joined := strings.Join(build[1:], " ")
	for _, want := range []string{"build", "--release", "--target " + TargetTriple, "--manifest-path " + cfg.Manifest(), "--features " + DefaultFeatures} {
		if !strings.Contains(joined, want) {
			t.Errorf("build call %q is missing %q", joined, want)
		}
	}
	for _, call := range calls {
		if strings.Contains(strings.Join(call[1:], " "), "--features") && !strings.Contains(strings.Join(call[1:], " "), "build") {
			t.Errorf("only the build itself may carry --features: %v", call)
		}
	}
	if stages := progress.stageList(); len(stages) != 2 || stages[0] != 1 || stages[1] != 2 {
		t.Errorf("stages = %v, want 1 then 2", stages)
	}
	if !progress.sawLog("個區塊") {
		t.Error("the block count was never logged")
	}
}

func TestBuildWithoutRustupSkipsTheTargetInstall(t *testing.T) {
	cfg := writeFirmware(t, flashELF())
	cfg.CargoPath = fakeCargo(t, false)
	runner := &fakeRunner{}
	progress := &recordingProgress{}
	if _, err := Build(cfg, runner, NeverCancel, progress); err != nil {
		t.Fatalf("build: %v", err)
	}
	if calls := runner.commands(); len(calls) != 1 {
		t.Fatalf("calls = %v, want only cargo", calls)
	}
	if !progress.sawLog("找不到 rustup") {
		t.Error("the skipped target install was not logged")
	}
}

func TestBuildReportsAMissingFirmwareFolderBeforeCargo(t *testing.T) {
	cfg := FlashConfig{ManifestDir: filepath.Join(t.TempDir(), "nowhere"), CargoPath: fakeCargo(t, false)}
	runner := &fakeRunner{}
	_, err := Build(cfg, runner, NeverCancel, nil)
	if err == nil || !strings.Contains(messageOf(err), "app/Cargo.toml") {
		t.Fatalf("err = %v, want a missing app/Cargo.toml message", err)
	}
	if len(runner.commands()) != 0 {
		t.Error("cargo ran for a folder that does not exist")
	}
}

func TestBuildRejectsACargoPathThatDoesNotExist(t *testing.T) {
	cfg := writeFirmware(t, flashELF())
	cfg.CargoPath = filepath.Join(t.TempDir(), exeName("no-cargo"))
	_, err := Build(cfg, &fakeRunner{}, NeverCancel, nil)
	if CodeOf(err) != Invalid || !strings.Contains(messageOf(err), "不存在") {
		t.Fatalf("err = %v, want INVALID / 不存在", err)
	}
}

func TestBuildReportsAFailingCargoAndTheMissingElf(t *testing.T) {
	cfg := writeFirmware(t, nil)
	cfg.CargoPath = fakeCargo(t, false)
	runner := &fakeRunner{failOn: "--features"}
	_, err := Build(cfg, runner, NeverCancel, nil)
	if CodeOf(err) != IO || !strings.Contains(messageOf(err), "子程序失敗") {
		t.Fatalf("err = %v, want IO / 子程序失敗", err)
	}

	// A cargo that claims success but left no artifact is reported, not packed.
	cfg = writeFirmware(t, nil)
	cfg.CargoPath = fakeCargo(t, false)
	_, err = Build(cfg, &fakeRunner{}, NeverCancel, nil)
	if CodeOf(err) != Invalid || !strings.Contains(messageOf(err), "找不到 ELF") {
		t.Fatalf("err = %v, want INVALID / 找不到 ELF", err)
	}
}

func TestBuildStopsOnACancelledToken(t *testing.T) {
	cfg := writeFirmware(t, flashELF())
	cfg.CargoPath = fakeCargo(t, true)
	runner := &fakeRunner{}
	if _, err := Build(cfg, runner, CancelFunc(func() bool { return true }), nil); CodeOf(err) != Cancelled {
		t.Fatalf("err = %v, want CANCELLED", err)
	}
	if len(runner.commands()) != 0 {
		t.Error("a cancelled build still ran cargo")
	}
}

// --- push -------------------------------------------------------------------

// uf2Drive creates a directory that looks like a mounted RP2040 bootloader.
func uf2Drive(t *testing.T) string {
	t.Helper()
	drive := t.TempDir()
	info := "UF2 Bootloader v3.0\nModel: Raspberry Pi RP2\nBoard-ID: RPI-RP2\n"
	if err := os.WriteFile(filepath.Join(drive, "INFO_UF2.TXT"), []byte(info), 0o644); err != nil {
		t.Fatalf("write INFO_UF2.TXT: %v", err)
	}
	return drive
}

func TestPushSendsBootloaderWaitsForTheDriveAndVerifies(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	drive := uf2Drive(t)
	polls := 0
	enumerations := 0
	flashSeams(t,
		func() ([]string, error) {
			polls++
			if polls == 1 {
				return nil, nil // nothing mounted before BOOTLOADER
			}
			return []string{drive}, nil
		},
		func() ([]Entry, error) {
			enumerations++
			// The snapshot taken before the copy sees nothing: the board is in
			// the bootloader, not in runtime.
			if enumerations == 1 {
				return nil, nil
			}
			return []Entry{{Path: "re-enumerated", Serial: "MOCK"}}, nil
		},
		func(entry Entry, boardID uint8) (Verified, error) {
			if entry.Path != "re-enumerated" {
				t.Errorf("verified %q, want the re-enumerated board", entry.Path)
			}
			if boardID != BoardID {
				t.Errorf("board id = %d, want %d", boardID, BoardID)
			}
			return Verified{Board: BoardID, Firmware: "3.2.0", LeaseMS: 2000}, nil
		},
	)
	pollInterval = time.Millisecond
	handshakeWait = 2 * time.Second
	image := Image{Bytes: bytes.Repeat([]byte{0xA5}, 3*UF2BlockSize), Blocks: 3}
	progress := &recordingProgress{}
	outcome, err := Push(device, image, progress, NeverCancel)
	if err != nil {
		t.Fatalf("push: %v", err)
	}
	if !outcome.OK() || outcome.Verified.Board != BoardID || outcome.Verified.Firmware != "3.2.0" {
		t.Fatalf("outcome = %+v", outcome)
	}
	if !strings.Contains(outcome.Label(), "傳輸完成且握手通過") {
		t.Errorf("label = %q", outcome.Label())
	}
	if stages := progress.stageList(); len(stages) != 3 || stages[0] != 3 || stages[1] != 4 || stages[2] != 5 {
		t.Fatalf("stages = %v, want 3, 4, 5", stages)
	}
	// BOOTLOADER went out, then CLOSE took the runtime session away before the
	// drive was waited for.
	bootloaders := 0
	for _, packet := range handle.Writes() {
		if packet.Opcode == OpBootloader {
			bootloaders++
		}
	}
	if bootloaders != 1 {
		t.Errorf("BOOTLOADER frames = %d, want 1", bootloaders)
	}
	if !handle.Closed() {
		t.Error("the runtime session was not closed before waiting for the drive")
	}
	copied, err := os.ReadFile(filepath.Join(drive, "Firmware.uf2"))
	if err != nil {
		t.Fatalf("read copied image: %v", err)
	}
	if !bytes.Equal(copied, image.Bytes) {
		t.Errorf("copied %d bytes, want %d", len(copied), len(image.Bytes))
	}
	if !progress.sawLog("已確認 enter_bootloader") {
		t.Error("the confirmed BOOTLOADER was not logged")
	}
}

func TestPushCopiesIntoAnExplicitBootTargetWithoutAsking(t *testing.T) {
	device, handle := openTestDevice(t, quietOptions())
	drive := uf2Drive(t)
	flashSeams(t,
		func() ([]string, error) {
			t.Error("an explicit boot target must not use volume detection")
			return nil, nil
		},
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) {
			return Verified{}, errorf(Timeout, "delayed handshake")
		},
	)
	pollInterval = time.Millisecond
	handshakeWait = 100 * time.Millisecond
	image := Image{Bytes: []byte("uf2"), Blocks: 1}
	outcome, err := PushWithConfig(FlashConfig{BootTarget: drive}, device, image, &recordingProgress{}, NeverCancel)
	if err != nil {
		t.Fatalf("push: %v", err)
	}
	// The transfer landed; verification did not, which is reported not failed.
	if outcome.OK() {
		t.Fatalf("outcome = %+v, want unverified", outcome)
	}
	if CodeOf(outcome.Unverified) != Timeout {
		t.Fatalf("unverified = %v, want TIMEOUT", outcome.Unverified)
	}
	if !strings.Contains(outcome.Label(), "傳輸完成，但未驗證") {
		t.Errorf("label = %q", outcome.Label())
	}
	for _, packet := range handle.Writes() {
		if packet.Opcode == OpBootloader {
			t.Error("an explicit boot target still sent BOOTLOADER")
		}
	}
	copied, err := os.ReadFile(filepath.Join(drive, "Firmware.uf2"))
	if err != nil || !bytes.Equal(copied, image.Bytes) {
		t.Fatalf("copied = %q, %v", copied, err)
	}
}

func TestPushWithoutADeviceAsksForPhysicalBootselAndTimesOut(t *testing.T) {
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	driveWait = 40 * time.Millisecond
	pollInterval = time.Millisecond
	progress := &recordingProgress{}
	_, err := Push(nil, Image{Bytes: []byte("uf2"), Blocks: 1}, progress, NeverCancel)
	if CodeOf(err) != Timeout {
		t.Fatalf("err = %v, want TIMEOUT", err)
	}
	if !strings.Contains(messageOf(err), "未偵測到 RP2040 UF2 磁碟") {
		t.Errorf("message = %q, want the drive timeout wording", messageOf(err))
	}
	if !progress.sawLog("請使用實體 BOOTSEL") {
		t.Error("the physical BOOTSEL hint was not logged")
	}
	if stages := progress.stageList(); len(stages) != 1 || stages[0] != 3 {
		t.Errorf("stages = %v, want only 3", stages)
	}
}

func TestPushRejectsAnythingThatIsNotABootloaderDisk(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	notADrive := t.TempDir()
	_, err := PushWithConfig(FlashConfig{BootTarget: notADrive}, device, Image{Bytes: []byte("uf2")}, nil, NeverCancel)
	if CodeOf(err) != Invalid || !strings.Contains(messageOf(err), "UF2 bootloader") {
		t.Fatalf("err = %v, want INVALID / UF2 bootloader", err)
	}
	if _, statErr := os.Stat(filepath.Join(notADrive, "Firmware.uf2")); statErr == nil {
		t.Error("the image was copied into a directory that is not a bootloader disk")
	}
}

func TestPushWillNotUseAnAlreadyRunningBoardForVerification(t *testing.T) {
	device, _ := openTestDevice(t, quietOptions())
	drive := uf2Drive(t)
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return []Entry{{Path: "already-running", Serial: "MOCK"}}, nil },
		func(Entry, uint8) (Verified, error) {
			t.Error("an already-running board was opened for verification")
			return Verified{}, nil
		},
	)
	handshakeWait = 40 * time.Millisecond
	pollInterval = time.Millisecond
	outcome, err := PushWithConfig(FlashConfig{BootTarget: drive}, device, Image{Bytes: []byte("uf2")}, nil, NeverCancel)
	if err != nil {
		t.Fatalf("push: %v", err)
	}
	if outcome.OK() {
		t.Fatalf("outcome = %+v, want unverified", outcome)
	}
	if CodeOf(outcome.Unverified) != Timeout {
		t.Errorf("unverified = %v, want TIMEOUT", outcome.Unverified)
	}
}

// --- verification -----------------------------------------------------------

func TestCheckBoardOnlyAcceptsTheFlashedBoard(t *testing.T) {
	verified, err := checkBoard(Info{Board: BoardID, Firmware: "3.2.0", LeaseMS: 2000}, BoardID)
	if err != nil {
		t.Fatalf("the XIAO build did not verify: %v", err)
	}
	if verified.Board != 2 || verified.Firmware != "3.2.0" || verified.LeaseMS != 2000 {
		t.Errorf("verified = %+v", verified)
	}
	if !strings.Contains(verified.String(), "board=2, fw=3.2.0, lease=2000 ms") {
		t.Errorf("String() = %q", verified.String())
	}
	for _, board := range []uint8{1, 3} {
		if _, err := checkBoard(Info{Board: board, Firmware: "3.2.0"}, BoardID); CodeOf(err) != Invalid {
			t.Errorf("board %d verified as a XIAO: %v", board, err)
		}
	}
	if _, err := checkBoard(Info{Board: BoardID}, BoardID); CodeOf(err) != Invalid {
		t.Errorf("a board without a firmware version verified: %v", err)
	}
}

func TestWaitForBoardRetriesTheHandshakeAndRejectsAmbiguity(t *testing.T) {
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return []Entry{{Path: "new", Serial: "serial"}}, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	pollInterval = time.Millisecond
	tries := 0
	openForVerify = func(Entry, uint8) (Verified, error) {
		tries++
		if tries == 1 {
			return Verified{}, errorf(Timeout, "delayed handshake")
		}
		return Verified{Board: BoardID, Firmware: "3.2.0", LeaseMS: 2000}, nil
	}
	verified, err := waitForBoard(NeverCancel, time.Second, "serial", BoardID, nil)
	if err != nil {
		t.Fatalf("wait: %v", err)
	}
	if verified.Board != BoardID || tries != 2 {
		t.Fatalf("verified = %+v after %d tries", verified, tries)
	}

	enumerateDevices = func() ([]Entry, error) {
		return []Entry{{Path: "a", Serial: "shared"}, {Path: "b", Serial: "shared"}}, nil
	}
	openForVerify = func(Entry, uint8) (Verified, error) {
		t.Error("two matching boards cannot be told apart; the first must not be picked")
		return Verified{}, nil
	}
	if _, err := waitForBoard(NeverCancel, 40*time.Millisecond, "", BoardID, nil); CodeOf(err) != Timeout {
		t.Fatalf("err = %v, want TIMEOUT", err)
	}
}

func TestWaitForBoardTimesOutQuicklyWhenNothingComesBack(t *testing.T) {
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	pollInterval = time.Millisecond
	started := time.Now()
	_, err := waitForBoard(NeverCancel, 40*time.Millisecond, "", BoardID, nil)
	if CodeOf(err) != Timeout {
		t.Fatalf("err = %v, want TIMEOUT", err)
	}
	if elapsed := time.Since(started); elapsed > time.Second {
		t.Errorf("the wait took %v, want it bounded by the timeout", elapsed)
	}
	if !strings.Contains(messageOf(err), "runtime hello 驗證逾時") {
		t.Errorf("message = %q", messageOf(err))
	}
}

func TestWaitForBoardHonoursCancellation(t *testing.T) {
	flashSeams(t,
		func() ([]string, error) { return nil, nil },
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	pollInterval = time.Millisecond
	cancelled := CancelFunc(func() bool { return true })
	if _, err := waitForBoard(cancelled, time.Second, "", BoardID, nil); CodeOf(err) != Cancelled {
		t.Fatalf("err = %v, want CANCELLED", err)
	}
}

// --- copy -------------------------------------------------------------------

func TestWriteImageKeepsTheOsErrorAndACancelledCopyDoesNotLookFinished(t *testing.T) {
	root := t.TempDir()
	missing := filepath.Join(root, "missing", "Firmware.uf2")
	err := writeImage(missing, []byte{1, 2, 3, 4}, NeverCancel)
	if CodeOf(err) != IO || !strings.Contains(messageOf(err), "UF2") {
		t.Fatalf("err = %v, want IO / UF2", err)
	}

	target := filepath.Join(root, "Firmware.uf2")
	if err := writeImage(target, []byte{1, 2, 3, 4}, CancelFunc(func() bool { return true })); CodeOf(err) != Cancelled {
		t.Fatalf("err = %v, want CANCELLED", err)
	}
	if _, statErr := os.Stat(target); statErr == nil {
		t.Fatal("a cancelled copy left a file that looks finished")
	}
}

func TestWriteImageCopiesInChunks(t *testing.T) {
	target := filepath.Join(t.TempDir(), "Firmware.uf2")
	image := bytes.Repeat([]byte{0x11}, CopyChunk*2+7)
	if err := writeImage(target, image, NeverCancel); err != nil {
		t.Fatalf("write: %v", err)
	}
	written, err := os.ReadFile(target)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	if !bytes.Equal(written, image) {
		t.Fatalf("wrote %d bytes, want %d", len(written), len(image))
	}
}

func TestPushRefusesToGuessBetweenTwoDrives(t *testing.T) {
	first, second := t.TempDir(), t.TempDir()
	for _, drive := range []string{first, second} {
		if err := os.WriteFile(filepath.Join(drive, "INFO_UF2.TXT"), []byte("RPI-RP2"), 0o644); err != nil {
			t.Fatalf("prepare drive: %v", err)
		}
	}
	flashSeams(t,
		func() ([]string, error) { return []string{first, second}, nil },
		func() ([]Entry, error) { return nil, nil },
		func(Entry, uint8) (Verified, error) { return Verified{}, nil },
	)
	pollInterval = time.Millisecond
	_, err := Push(nil, Image{Bytes: []byte("uf2")}, &recordingProgress{}, NeverCancel)
	if CodeOf(err) != Invalid || !strings.Contains(messageOf(err), "同時偵測到多個 UF2 磁碟") {
		t.Fatalf("err = %v, want INVALID / multiple drives", err)
	}
	for _, drive := range []string{first, second} {
		if _, statErr := os.Stat(filepath.Join(drive, "Firmware.uf2")); statErr == nil {
			t.Errorf("a drive was written without asking: %s", drive)
		}
	}
}

func TestProgressFuncsRefusesByDefaultAndSystemRunnerStreams(t *testing.T) {
	if _, err := (ProgressFuncs{}).ChooseDrive([]string{"E:\\", "F:\\"}); CodeOf(err) != Invalid {
		t.Fatalf("err = %v, want INVALID", err)
	}
	// A child that prints on both pipes is streamed line by line, so the log
	// carries cargo's output while it runs rather than at the end.
	program := "/bin/sh"
	args := []string{"-c", "echo out; echo err >&2"}
	if runtime.GOOS == "windows" {
		program = "cmd"
		args = []string{"/c", "echo out & echo err 1>&2"}
	}
	if _, err := os.Stat(program); err != nil {
		program = exeName("cmd")
	}
	var seen []string
	result, err := (SystemRunner{}).Run(program, args, 10*time.Second, NeverCancel, func(stream Stream, text string) {
		seen = append(seen, stream.String()+": "+text)
	})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if _, err := result.Check(); err != nil {
		t.Fatalf("check: %v", err)
	}
	joined := strings.Join(seen, "|")
	if !strings.Contains(joined, "out: out") || !strings.Contains(joined, "err: err") {
		t.Errorf("streamed lines = %v", seen)
	}
}
