# Installing RapidOne

Download the latest build for your system from the release page. The AI models for
Edit (subject masks, denoise, Remove Lite, finding objects) are inside the installer, so
those work straight away, offline. The **Retouch** tab downloads its AI engine the first
time it opens: about 3 GB with an NVIDIA graphics card, about 1 GB without, once.

These builds aren't code-signed yet, so your system may warn that the publisher is
unverified on first launch. That's expected, not a sign anything's wrong.

## Linux

Two downloads, the same app:

- **`.AppImage`** (any recent distribution: Ubuntu 24.04 or newer, Linux Mint 22, Pop!_OS
  24.04, Fedora 40+, Debian 13, Arch and others; older ones such as Ubuntu 22.04 aren't
  supported). Make it executable and run it:
  ```
  chmod +x RapidOne_*.AppImage
  ./RapidOne_*.AppImage
  ```
  On **Ubuntu 24.04 and newer** an AppImage needs FUSE 2, which isn't installed by
  default. If it doesn't start, install it once:
  ```
  sudo apt install libfuse2t64
  ```
- **`.deb`** (Ubuntu, Mint, Pop!_OS, Debian): installs RapidOne with a menu entry, and
  uninstalls cleanly. No FUSE needed:
  ```
  sudo apt install ./RapidOne_*.deb
  ```

**If the window comes up blank or the app closes straight away** (some graphics
drivers), start it once with the safe drawing mode, which is slower but works
everywhere:
```
RAPIDONE_SAFE_DRAWING=1 ./RapidOne_*.AppImage
```

**Culling from your phone:** if the phone can't find the computer, your firewall may be
blocking it. Allow ports 47811-47850 on your home network (with ufw:
`sudo ufw allow from 192.168.0.0/16 to any port 47811:47850 proto tcp`).

## Windows

Run the `.exe` installer (or the `.msi`). SmartScreen will show "Windows protected your
PC": click **More info**, then **Run anyway**.

## macOS (Apple Silicon)

Open the `.dmg` and drag RapidOne into Applications. The first time, macOS will refuse
to open it. Either run this once in Terminal, then open it normally:
```
xattr -d com.apple.quarantine /Applications/RapidOne.app
```
or try to open it once, then go to **System Settings → Privacy & Security** and click
**Open Anyway** next to the notice about RapidOne.
