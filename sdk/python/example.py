"""Same walkthrough as the Rust example, through the ctypes binding.

    python sdk/python/example.py
"""

import gdb


def main() -> None:
    with gdb.Device.open_mock() as device:
        print("info:", device.info)
        print("status:", device.status)

        device.keyboard.type_text("hello", interval_ms=1, hold_ms=1, jitter_ms=0)
        device.keyboard.chord([0xE0, 0x06], 20)

        mouse = device.mouse
        mouse.move_by(120, -40, duration_ms=30, profile=gdb.Profile.linear())
        mouse.move_to(400, 300)
        mouse.move_path([(100, 100), (320, 240)], duration_ms=40)
        mouse.click(gdb.Button.LEFT)
        mouse.wheel(120)

        with mouse.start_move_to(800, 600, duration_ms=300) as movement:
            movement.retarget(900, 650)
            movement.wait()

        with device.scheduler() as scheduler:
            scheduler.pause()
            scheduler.submit(0, 0, lambda: 0)
            scheduler.submit(0, 10, lambda: 0)
            scheduler.resume()
            scheduler.wait_idle(2000)
            print("scheduler errors:", scheduler.take_errors())

    print("python mock ok")


if __name__ == "__main__":
    main()
