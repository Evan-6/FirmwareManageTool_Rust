package gdbnative

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

// Firmware build and flashing for the XIAO RP2040, host side — the Go port of
// sdk/rust/src/flash.rs.
//
// Three things learned on hardware decide the shape of this file:
//
//   - the board refuses BOOTLOADER while a host still polls it, so the runtime
//     session is closed *before* the drive is waited for;
//   - the UF2 drive only appears after that reboot, so "was it mounted before
//     this command" — not its letter — is what identifies the right one;
//   - verification is a second enumeration plus a Feature 14 read, not the reply
//     of the copy: the old handle is gone by the time the board is back.
//
// Nothing here knows about a GUI: cargo comes in through Runner, progress
// through Progress and cancellation through Cancel, so the same sequence runs
// from any host program. The messages are the ones a person reads while
// flashing, hence Chinese.

// Crate name of the firmware binary this package builds.
const AppCrate = "xiao-firmware"

// Only supported firmware target: Seeed XIAO RP2040 (Cortex-M0+).
const TargetTriple = "thumbv6m-none-eabi"

// Feature that turns on the embassy USB device; without it the crate is a lib.
const DefaultFeatures = "hardware"

// BoardID is the feature `board` id reported by the XIAO firmware build.
const BoardID = 2

// Timings and sizes of the flashing sequence, the same ones sdk/rust/src/flash.rs
// uses. The exported constants are the defaults; the tests shorten the two waits
// through the vars below them.
const (
	// DriveWait is how long the UF2 drive may take to appear after the reboot.
	DriveWait = 30 * time.Second
	// HandshakeWait is how long the runtime HID may take to come back and
	// answer Feature 14.
	HandshakeWait = 15 * time.Second
	// CopyChunk is the copy granularity: a cancelled transfer stops between
	// chunks, not mid-block.
	CopyChunk = 16 * 1024
	// PollInterval is the "is it there yet" poll interval.
	PollInterval = 250 * time.Millisecond
	// retainedOutput bounds what a child's stdout/stderr keep in memory; every
	// line still reaches the log.
	retainedOutput = 10 * 1024 * 1024
)

// FlashConfig is where the firmware lives, how the board should be put into
// BOOTSEL and how the flashed firmware is identified.
type FlashConfig struct {
	// CargoPath is an explicit cargo binary; empty looks one up on PATH.
	CargoPath string
	// ManifestDir is the firmware root: the folder that contains app/.
	ManifestDir string
	// Target is the Rust target triple; empty means TargetTriple.
	Target string
	// Features is the cargo feature set; empty means DefaultFeatures.
	Features string
	// BootTarget is an explicit UF2 drive root such as "E:\"; empty waits for
	// the drive this command created. BOOTLOADER is only sent when it is empty.
	BootTarget string
	// BoardID is the board id the flashed firmware must report; 0 means BoardID.
	BoardID uint8
}

// DefaultFlashConfig is the repository layout: ./firmware/app, the XIAO target,
// the hardware feature and board 2.
func DefaultFlashConfig() FlashConfig {
	return FlashConfig{ManifestDir: "firmware", Target: TargetTriple, Features: DefaultFeatures, BoardID: BoardID}
}

// withDefaults fills the empty fields, so a zero FlashConfig is usable.
func (c FlashConfig) withDefaults() FlashConfig {
	if c.ManifestDir == "" {
		c.ManifestDir = "firmware"
	}
	if c.Target == "" {
		c.Target = TargetTriple
	}
	if c.Features == "" {
		c.Features = DefaultFeatures
	}
	if c.BoardID == 0 {
		c.BoardID = BoardID
	}
	return c
}

// AppDir is the cargo workspace of the firmware.
func (c FlashConfig) AppDir() string { return filepath.Join(c.ManifestDir, "app") }

// Manifest is app/Cargo.toml.
func (c FlashConfig) Manifest() string { return filepath.Join(c.AppDir(), "Cargo.toml") }

// ELF is the artifact cargo leaves behind: app/target/<target>/release/<crate>.
func (c FlashConfig) ELF() string {
	return filepath.Join(c.AppDir(), "target", c.withDefaults().Target, "release", AppCrate)
}

// Image is a packed image: the bytes the bootloader drive expects and their
// block count.
type Image struct {
	Bytes  []byte
	Blocks int
	// ELF is the artifact the image was packed from, for the log line a failure
	// prints.
	ELF string
}

// Cancel is cancellation as the flashing sequence needs it: one question, asked
// often. The host program owns the token — an atomic, a channel, a GUI flag —
// and this package never sleeps through it without asking.
type Cancel interface {
	Cancelled() bool
}

// CancelFunc adapts a plain func to Cancel.
type CancelFunc func() bool

// Cancelled calls the func.
func (f CancelFunc) Cancelled() bool { return f() }

// NeverCancel is the token to pass when the caller cannot be interrupted.
var NeverCancel Cancel = CancelFunc(func() bool { return false })

// Progress is what the sequence reports while it runs, in the order the user
// sees it: one log line at a time, the numbered stage it just entered, and the
// drive question when several UF2 disks are mounted.
//
// The stage numbers are the Rust SDK's: 1 check the toolchain, 2 compile,
// 3 enter and pick the bootloader, 4 transfer, 5 wait for the runtime handshake.
type Progress interface {
	Log(line string)
	Stage(stage uint8, message string)
	// ChooseDrive is asked when more than one candidate is mounted and the
	// answer must be one of candidates. Refusing is the default: guessing here
	// flashes the wrong board.
	ChooseDrive(candidates []string) (string, error)
}

// ProgressFuncs adapts three optional closures to Progress. The zero value is
// silent and refuses to guess a drive.
type ProgressFuncs struct {
	OnLog         func(string)
	OnStage       func(uint8, string)
	OnChooseDrive func([]string) (string, error)
}

// Log forwards to OnLog.
func (p ProgressFuncs) Log(line string) {
	if p.OnLog != nil {
		p.OnLog(line)
	}
}

// Stage forwards to OnStage.
func (p ProgressFuncs) Stage(stage uint8, message string) {
	if p.OnStage != nil {
		p.OnStage(stage, message)
	}
}

// ChooseDrive forwards to OnChooseDrive; without one it refuses, because
// flashing the wrong board is worse than stopping.
func (p ProgressFuncs) ChooseDrive(candidates []string) (string, error) {
	if p.OnChooseDrive != nil {
		return p.OnChooseDrive(candidates)
	}
	return "", errorf(Invalid, "同時偵測到多個 UF2 磁碟（%s），請指定 boot 磁碟", strings.Join(candidates, "、"))
}

// Stream is which pipe a child line came from.
type Stream int

const (
	// Stdout is the child's standard output.
	Stdout Stream = iota
	// Stderr is the child's standard error.
	Stderr
)

// String is the name sdk/rust/src/flash.rs prints in front of every child line.
func (s Stream) String() string {
	if s == Stderr {
		return "Stderr"
	}
	return "Stdout"
}

// RunResult is what a finished child program reported.
type RunResult struct {
	Stdout    string
	Stderr    string
	Exit      *int
	TimedOut  bool
	Cancelled bool
}

// Check returns stdout when the program succeeded, a usable error when not.
func (r RunResult) Check() (string, error) {
	if r.Cancelled {
		return "", errorf(Cancelled, "操作已取消")
	}
	if r.TimedOut {
		return "", errorf(Timeout, "子程序逾時")
	}
	if r.Exit == nil || *r.Exit != 0 {
		return "", errorf(IO, "子程序失敗，exit=%s\n%s\n%s", formatExit(r.Exit), r.Stderr, r.Stdout)
	}
	return r.Stdout, nil
}

func formatExit(exit *int) string {
	if exit == nil {
		return "none"
	}
	return fmt.Sprintf("%d", *exit)
}

// Runner runs an external program. The host supplies its own when it already
// has one (the desktop tool kills a whole process tree, which a library cannot
// assume).
type Runner interface {
	Run(program string, args []string, timeout time.Duration, cancel Cancel, log func(Stream, string)) (RunResult, error)
}

// SystemRunner is the plain implementation: spawn, stream, kill on cancel or
// timeout.
type SystemRunner struct{}

type childLine struct {
	stream Stream
	text   string
}

// Run starts program, streams both pipes line by line and stops it on cancel or
// timeout.
func (SystemRunner) Run(program string, args []string, timeout time.Duration, cancel Cancel, log func(Stream, string)) (RunResult, error) {
	var result RunResult
	command := exec.Command(program, args...)
	stdout, err := command.StdoutPipe()
	if err != nil {
		return result, errorf(IO, "無法啟動 %s：%s", program, err)
	}
	stderr, err := command.StderrPipe()
	if err != nil {
		return result, errorf(IO, "無法啟動 %s：%s", program, err)
	}
	if err := command.Start(); err != nil {
		return result, errorf(IO, "無法啟動 %s：%s", program, err)
	}
	if cancel == nil {
		cancel = NeverCancel
	}
	if log == nil {
		log = func(Stream, string) {}
	}

	lines := make(chan childLine, 128)
	stop := make(chan struct{})
	defer close(stop)
	var readers sync.WaitGroup
	readers.Add(2)
	go scanLines(stdout, Stdout, lines, stop, &readers)
	go scanLines(stderr, Stderr, lines, stop, &readers)
	go func() {
		readers.Wait()
		close(lines)
	}()
	done := make(chan error, 1)
	go func() { done <- command.Wait() }()

	pending := (<-chan childLine)(lines)
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	started := time.Now()
	var waitErr error
	for {
		select {
		case line, ok := <-pending:
			if !ok {
				pending = nil
				continue
			}
			absorb(&result, line, log)
		case err := <-done:
			waitErr = err
			done = nil
			goto finished
		case <-ticker.C:
			if cancel.Cancelled() || time.Since(started) >= timeout {
				result.Cancelled = cancel.Cancelled()
				result.TimedOut = !result.Cancelled
				_ = command.Process.Kill()
				waitErr = <-done
				done = nil
				goto finished
			}
		}
	}

finished:
	// A grandchild can keep the pipes open after the child is gone, so the
	// readers are given a moment and then left to finish on their own.
	grace := time.NewTimer(time.Second)
	defer grace.Stop()
	for pending != nil {
		select {
		case line, ok := <-pending:
			if !ok {
				pending = nil
				continue
			}
			absorb(&result, line, log)
		case <-grace.C:
			pending = nil
		}
	}
	result.Exit = exitCode(waitErr)
	return result, nil
}

// absorb moves one line into the result and the log; the retained text is
// bounded, the log is not.
func absorb(result *RunResult, line childLine, log func(Stream, string)) {
	sink := &result.Stdout
	if line.stream == Stderr {
		sink = &result.Stderr
	}
	if len(*sink)+len(line.text) <= retainedOutput {
		*sink += line.text + "\n"
	}
	log(line.stream, line.text)
}

// scanLines turns one pipe into lines, dropping the trailing newline. It stops
// when the pipe ends or when the caller no longer reads.
func scanLines(reader io.Reader, stream Stream, out chan<- childLine, stop <-chan struct{}, done *sync.WaitGroup) {
	defer done.Done()
	buffered := bufio.NewReaderSize(reader, 64*1024)
	for {
		text, err := buffered.ReadString('\n')
		if text != "" {
			select {
			case out <- childLine{stream: stream, text: strings.TrimRight(text, "\r\n")}:
			case <-stop:
				return
			}
		}
		if err != nil {
			return
		}
	}
}

// exitCode is the child's exit status, nil when the process never reported one.
func exitCode(waitErr error) *int {
	if waitErr == nil {
		code := 0
		return &code
	}
	var failure *exec.ExitError
	if errors.As(waitErr, &failure) {
		code := failure.ExitCode()
		return &code
	}
	return nil
}

// Build builds app/ for the RP2040 and packs the result into a UF2 image.
func Build(cfg FlashConfig, runner Runner, cancel Cancel, progress Progress) (Image, error) {
	var image Image
	cfg = cfg.withDefaults()
	if runner == nil {
		runner = SystemRunner{}
	}
	if cancel == nil {
		cancel = NeverCancel
	}
	if progress == nil {
		progress = ProgressFuncs{}
	}
	if err := checkCancel(cancel); err != nil {
		return image, err
	}
	if info, err := os.Stat(cfg.Manifest()); err != nil || info.IsDir() {
		return image, errorf(Invalid, "韌體資料夾不完整（缺 app/Cargo.toml）：%s", cfg.ManifestDir)
	}
	progress.Stage(1, "檢查 Rust 工具鏈")
	cargo, err := cargoPath(cfg)
	if err != nil {
		return image, err
	}
	if rustup := rustupBeside(cargo); rustup != "" {
		if _, err := run(runner, rustup, []string{"target", "add", cfg.Target}, 600*time.Second, cancel, progress); err != nil {
			return image, err
		}
	} else {
		progress.Log(fmt.Sprintf("找不到 rustup，跳過 target 安裝（假設 %s 已安裝）", cfg.Target))
	}
	progress.Stage(2, "編譯 Rust 韌體")
	build := []string{
		"build",
		"--release",
		"--target", cfg.Target,
		"--manifest-path", cfg.Manifest(),
		"--features", cfg.Features,
	}
	if _, err := run(runner, cargo, build, 900*time.Second, cancel, progress); err != nil {
		return image, err
	}
	elf := cfg.ELF()
	if info, err := os.Stat(elf); err != nil || info.IsDir() {
		return image, errorf(Invalid, "編譯成功但找不到 ELF：%s", elf)
	}
	raw, err := os.ReadFile(elf)
	if err != nil {
		return image, errorf(IO, "讀取 ELF 失敗：%s：%s", elf, err)
	}
	packed, err := UFPack(raw)
	if err != nil {
		return image, err
	}
	image = Image{Bytes: packed, Blocks: UF2Blocks(packed), ELF: elf}
	progress.Log(fmt.Sprintf("UF2：%d 個區塊", image.Blocks))
	return image, nil
}

// Verified is what the runtime board answered after the reboot.
type Verified struct {
	Board    uint8
	Firmware string
	LeaseMS  uint16
}

// String is the one-line summary the tool shows: "board=2, fw=3.2.0, lease=2000 ms".
func (v Verified) String() string {
	return fmt.Sprintf("board=%d, fw=%s, lease=%d ms", v.Board, v.Firmware, v.LeaseMS)
}

// Outcome is what a finished transfer left behind. A nil error means the bytes
// are on the flash; Verified is set when the board also came back and answered,
// Unverified carries the reason it did not — the distinction the caller reports
// to the user.
type Outcome struct {
	Verified   *Verified
	Unverified error
}

// OK reports whether the board came back and answered.
func (o Outcome) OK() bool { return o.Verified != nil }

// Label is the user-facing summary, the same wording src/firmware.rs uses.
func (o Outcome) Label() string {
	switch {
	case o.Verified != nil:
		return "傳輸完成且握手通過：" + o.Verified.String()
	case o.Unverified != nil:
		return "傳輸完成，但未驗證：" + messageOf(o.Unverified)
	default:
		return "傳輸完成，但未驗證"
	}
}

// Seams the flash flow reaches the outside world through. They are package
// variables so the headless tests can drive the whole sequence — a real drive,
// a real board and a real re-enumeration are exactly what a test must not need.
var (
	// listVolumes reports every mounted RP2040 bootloader drive root.
	listVolumes = volumes
	// enumerateDevices reports the runtime HID collections.
	enumerateDevices = List
	// openForVerify reopens one enumerated board and reads Feature 14.
	openForVerify = openAndCheck
	// driveWait and handshakeWait are the constants above, shortened by tests.
	driveWait     = DriveWait
	handshakeWait = HandshakeWait
	pollInterval  = PollInterval
)

// Push flashes image onto the board: BOOTLOADER, wait for the drive, copy,
// verify.
//
//	Err                 the transfer itself failed
//	Outcome.Verified    the copy finished and the board answered
//	Outcome.Unverified  the copy finished but the board never answered
//
// It uses DefaultFlashConfig, so an explicit boot drive is not possible; use
// PushWithConfig for that.
func Push(device *Device, image Image, progress Progress, cancel Cancel) (Outcome, error) {
	return PushWithConfig(DefaultFlashConfig(), device, image, progress, cancel)
}

// PushWithConfig is Push with the firmware layout and an explicit boot drive.
//
// device is the runtime handle BOOTLOADER is sent to; it is closed before the
// drive is waited for, because the board refuses the command while a host keeps
// polling it. A nil device means the board is already in BOOTSEL by hand.
func PushWithConfig(cfg FlashConfig, device *Device, image Image, progress Progress, cancel Cancel) (Outcome, error) {
	cfg = cfg.withDefaults()
	if progress == nil {
		progress = ProgressFuncs{}
	}
	if cancel == nil {
		cancel = NeverCancel
	}
	if err := checkCancel(cancel); err != nil {
		return Outcome{}, err
	}
	progress.Stage(3, "進入並選擇 bootloader")
	// Never auto-select a drive that was already mounted before this command:
	// it belongs to some other board.
	previous := []string{}
	if cfg.BootTarget == "" && device != nil {
		mounted, err := listVolumes()
		if err != nil {
			return Outcome{}, err
		}
		previous = mounted
	}
	if cfg.BootTarget == "" {
		if device == nil {
			progress.Log("未選取 runtime HID，請使用實體 BOOTSEL 進入下載模式")
		} else {
			if err := device.Bootloader(); err != nil {
				if err := checkCancel(cancel); err != nil {
					return Outcome{}, err
				}
				progress.Log(fmt.Sprintf("bootloader 命令未收到完整確認，繼續檢查列舉結果：%s", messageOf(err)))
			} else {
				progress.Log("已確認 enter_bootloader；等待裝置重啟")
			}
			// The board refuses BOOTLOADER while a host keeps polling it, and the
			// runtime session has to be gone before the drive shows up.
			_ = device.Close()
		}
	}
	drive := cfg.BootTarget
	if drive == "" {
		var err error
		drive, err = waitDrive(cancel, progress, previous)
		if err != nil {
			return Outcome{}, err
		}
	}
	if err := checkCancel(cancel); err != nil {
		return Outcome{}, err
	}
	// Boards already in runtime here are unrelated to this transfer: excluding
	// them stops an existing board's hello from verifying another board.
	existing := []string{}
	entries, err := enumerateDevices()
	if err != nil {
		return Outcome{}, err
	}
	for _, entry := range entries {
		existing = append(existing, entry.Path)
	}
	progress.Stage(4, "傳輸韌體")
	if info, err := os.Stat(filepath.Join(drive, "INFO_UF2.TXT")); err != nil || info.IsDir() {
		return Outcome{}, errorf(Invalid, "指定位置不是 UF2 bootloader 磁碟")
	}
	if err := writeImage(filepath.Join(drive, "Firmware.uf2"), image.Bytes, cancel); err != nil {
		return Outcome{}, err
	}
	progress.Log("UF2 複製完成，等待 runtime 驗證")
	progress.Stage(5, "等待 runtime HID 重新列舉與 hello 驗證")
	serial := ""
	if device != nil {
		serial = device.Serial()
	}
	verified, err := waitForBoard(cancel, handshakeWait, serial, cfg.BoardID, existing)
	if err != nil {
		return Outcome{Unverified: err}, nil
	}
	return Outcome{Verified: &verified}, nil
}

// openAndCheck reopens one enumerated board, reads Feature 14 and compares what
// the board says about itself with the build that was just written.
func openAndCheck(entry Entry, boardID uint8) (Verified, error) {
	device, err := Open(entry.Path, Options{TimeoutMS: 1000, AutoReconnect: false, Heartbeat: false})
	if err != nil {
		return Verified{}, err
	}
	defer func() { _ = device.Close() }()
	return checkBoard(device.Info(), boardID)
}

// checkBoard is the verification itself: a Pico (1) or AVR (3) build must not
// pass as the XIAO firmware that was just written.
func checkBoard(info Info, boardID uint8) (Verified, error) {
	if info.Board != boardID {
		return Verified{}, errorf(Invalid, "重新列舉的板型不符：%d（預期 %d）", info.Board, boardID)
	}
	if info.Firmware == "" {
		return Verified{}, errorf(Invalid, "重新列舉的裝置沒有回報韌體版本")
	}
	return Verified{Board: info.Board, Firmware: info.Firmware, LeaseMS: info.LeaseMS}, nil
}

// waitForBoard waits for the runtime HID to come back and answer, retrying
// until timeout. Two matching boards cannot prove which one was flashed, so
// that case is reported instead of picking the first.
func waitForBoard(cancel Cancel, timeout time.Duration, serial string, boardID uint8, existing []string) (Verified, error) {
	deadline := time.Now().Add(timeout)
	last := "尚未列舉"
	for time.Now().Before(deadline) {
		if err := checkCancel(cancel); err != nil {
			return Verified{}, err
		}
		entries, err := enumerateDevices()
		if err != nil {
			return Verified{}, err
		}
		matched := make([]Entry, 0, 1)
		for _, entry := range entries {
			if contains(existing, entry.Path) {
				continue
			}
			if serial != "" && entry.Serial != serial {
				continue
			}
			matched = append(matched, entry)
		}
		switch len(matched) {
		case 1:
			verified, err := openForVerify(matched[0], boardID)
			if err == nil {
				return verified, nil
			}
			last = messageOf(err)
		case 0:
		default:
			last = "多個符合裝置，無法確認燒錄目標；請僅連接目標板後驗證"
		}
		if err := pause(cancel); err != nil {
			return Verified{}, err
		}
	}
	return Verified{}, errorf(Timeout, "runtime hello 驗證逾時：%s", last)
}

// waitDrive waits for the bootloader volume this command created.
func waitDrive(cancel Cancel, progress Progress, previous []string) (string, error) {
	deadline := time.Now().Add(driveWait)
	for time.Now().Before(deadline) {
		if err := checkCancel(cancel); err != nil {
			return "", err
		}
		mounted, err := listVolumes()
		if err != nil {
			return "", err
		}
		fresh := make([]string, 0, len(mounted))
		for _, drive := range mounted {
			if !contains(previous, drive) {
				fresh = append(fresh, drive)
			}
		}
		switch len(fresh) {
		case 0:
		case 1:
			return fresh[0], nil
		default:
			return progress.ChooseDrive(fresh)
		}
		if err := pause(cancel); err != nil {
			return "", err
		}
	}
	return "", errorf(Timeout, "%d 秒內未偵測到 RP2040 UF2 磁碟；請 BOOTSEL 上電後重試", int(driveWait/time.Second))
}

// writeImage writes the image to target in chunks so a cancelled copy stops
// early instead of at the end.
func writeImage(target string, image []byte, cancel Cancel) error {
	if err := checkCancel(cancel); err != nil {
		return err
	}
	output, err := os.Create(target)
	if err != nil {
		return errorf(IO, "建立 UF2 目的檔失敗：%s：%s", target, err)
	}
	for start := 0; start < len(image); start += CopyChunk {
		if err := checkCancel(cancel); err != nil {
			_ = output.Close()
			_ = os.Remove(target)
			return err
		}
		stop := start + CopyChunk
		if stop > len(image) {
			stop = len(image)
		}
		if _, err := output.Write(image[start:stop]); err != nil {
			_ = output.Close()
			return errorf(IO, "UF2 複製失敗（保留 OS 錯誤碼）：%s：%s", target, err)
		}
	}
	if err := output.Sync(); err != nil {
		_ = output.Close()
		return errorf(IO, "UF2 重新整理失敗：%s：%s", target, err)
	}
	if err := output.Close(); err != nil {
		return errorf(IO, "UF2 重新整理失敗：%s：%s", target, err)
	}
	return nil
}

// run executes one child program and reports it as one log line plus its output.
func run(runner Runner, program string, args []string, timeout time.Duration, cancel Cancel, progress Progress) (string, error) {
	if err := checkCancel(cancel); err != nil {
		return "", err
	}
	progress.Log(program + " " + strings.Join(args, " "))
	result, err := runner.Run(program, args, timeout, cancel, func(stream Stream, text string) {
		progress.Log(stream.String() + ": " + text)
	})
	if err != nil {
		return "", err
	}
	if err := checkCancel(cancel); err != nil {
		return "", err
	}
	return result.Check()
}

// cargoPath is `cargo`, preferring an explicit setting.
func cargoPath(cfg FlashConfig) (string, error) {
	if cfg.CargoPath != "" {
		if info, err := os.Stat(cfg.CargoPath); err != nil || info.IsDir() {
			return "", errorf(Invalid, "指定的 cargo 不存在：%s", cfg.CargoPath)
		}
		return cfg.CargoPath, nil
	}
	name := "cargo"
	if runtime.GOOS == "windows" {
		name = "cargo.exe"
	}
	found, err := exec.LookPath(name)
	if err != nil {
		return "", errorf(Disconnected, "找不到 cargo；請安裝 Rust 工具鏈，或在設定指定 cargo 路徑")
	}
	return found, nil
}

// rustupBeside finds rustup next to cargo; without it the target cannot be
// added, which is not fatal when the target is already installed.
func rustupBeside(cargo string) string {
	name := "rustup"
	if runtime.GOOS == "windows" {
		name = "rustup.exe"
	}
	candidate := filepath.Join(filepath.Dir(cargo), name)
	if info, err := os.Stat(candidate); err == nil && !info.IsDir() {
		return candidate
	}
	return ""
}

// checkCancel is the one question the sequence asks before every step.
func checkCancel(cancel Cancel) error {
	if cancel != nil && cancel.Cancelled() {
		return errorf(Cancelled, "操作已取消")
	}
	return nil
}

// pause sleeps one poll interval in slices, so cancellation is honoured inside
// a wait.
func pause(cancel Cancel) error {
	end := time.Now().Add(pollInterval)
	for time.Now().Before(end) {
		if err := checkCancel(cancel); err != nil {
			return err
		}
		remaining := time.Until(end)
		if remaining > 10*time.Millisecond {
			remaining = 10 * time.Millisecond
		}
		if remaining > 0 {
			time.Sleep(remaining)
		}
	}
	return checkCancel(cancel)
}

func contains(list []string, value string) bool {
	for _, item := range list {
		if strings.EqualFold(item, value) {
			return true
		}
	}
	return false
}
