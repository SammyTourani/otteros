# Trying OtterOS on real hardware

The current build already boots on real x86_64 machines with UEFI or legacy BIOS. This smoke test
tells the build loop how your laptop behaves, long before the final demo needs it.

## You need
- An x86_64 laptop or PC (Intel or AMD). Apple Silicon Macs cannot run it.
- A USB stick whose contents you do not need. Writing the image erases it.

## 1. Build the image
    cd ~/Projects/otteros && source scripts/env.sh && gmake iso

## 2. Write it to the stick (macOS)
    diskutil list                      # find the stick by its size, e.g. /dev/disk4
    diskutil unmountDisk /dev/diskN
    sudo dd if=build/otteros.iso of=/dev/rdiskN bs=4m
    diskutil eject /dev/diskN
Check N twice. dd overwrites whichever disk you name.

## 3. Boot the laptop from it
- In the firmware setup (F2, F10, Del or Esc at power-on), disable Secure Boot. OtterOS's
  bootloader is not signed.
- Open the one-time boot menu (often F12, F9 or Esc) and choose the USB stick.
- OtterOS never writes to the laptop's own disk. Remove the stick and reboot to get your normal OS back.

## What you should see
- The Limine menu, then the OtterOS boot log: memory map, ACPI tables, timer calibration.
- A `>` prompt at the bottom. Typing should echo characters.

## Report back (tell Claude, or add to STATUS.md)
- Laptop make and model, and the CPU model.
- Did it reach the boot log? Did typing work?
- Does it have an Ethernet port, and which chip (Intel I219-V, Realtek RTL8111, ...)? A USB-Ethernet adapter also works later.
- A phone photo of the screen if anything looks wrong or it stops early.
