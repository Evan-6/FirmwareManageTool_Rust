// Same walkthrough as the other examples, through the Go binding.
//
//   set GDBOX_LIBRARY=D:\WorkSpace\FirmwareManageTool_Rust\target\debug\gdb.dll
//   go run ./sdk/go/examples/mock
package main

import (
	"fmt"
	"log"

	gdb "github.com/nousresearch/gdbox-sdk"
)

func main() {
	device, err := gdb.OpenMock()
	if err != nil {
		log.Fatal(err)
	}
	defer device.Close()

	info, err := device.InfoJSON()
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println("info:", info)

	if err := device.Keyboard().TypeText("hello", 1, 1, 0); err != nil {
		log.Fatal(err)
	}
	if err := device.Keyboard().Chord([]uint16{0xE0, 0x06}, 20); err != nil {
		log.Fatal(err)
	}

	profile := gdb.LinearProfile()
	if err := device.Mouse().MoveBy(120, -40, 30, &profile); err != nil {
		log.Fatal(err)
	}
	if err := device.Mouse().MoveTo(400, 300, 50, nil); err != nil {
		log.Fatal(err)
	}
	if err := device.Mouse().Click(gdb.Left, 1); err != nil {
		log.Fatal(err)
	}

	scheduler, err := device.Scheduler()
	if err != nil {
		log.Fatal(err)
	}
	defer scheduler.Close()
	if err := scheduler.Pause(); err != nil {
		log.Fatal(err)
	}
	if _, err := scheduler.Submit(0, 0, func() int32 { return 0 }); err != nil {
		log.Fatal(err)
	}
	if _, err := scheduler.Submit(0, 10, func() int32 { return 0 }); err != nil {
		log.Fatal(err)
	}
	if err := scheduler.Resume(); err != nil {
		log.Fatal(err)
	}
	if err := scheduler.WaitIdle(2000); err != nil {
		log.Fatal(err)
	}
	fmt.Println("go mock ok")
}
