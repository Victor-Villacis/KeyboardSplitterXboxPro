# Quickstart — install, choose a keyboard, and play

This is the customer path on Windows 11. It needs no terminal, no configuration
file, and no knowledge of the developer CLI.

## 1. Install ksx

Download `ksx-<version>-setup.exe` from the release page and run it.

- Leave **Install the ViGEmBus controller driver** selected unless it is
  already installed.
- Leave **Launch ksx** selected to open the app when setup finishes, or use the
  **ksx** shortcut afterward.
- The optional desktop icon and the Start-menu entry open the same app. Neither
  opens a command window.

The installer is not code-signed yet, so Windows may show a SmartScreen warning.
The release notes publish the installer SHA-256 so the download can be checked.

## 2. Choose the keyboard or arcade panel

ksx opens directly to **Setup** and scans the machine.

1. Find the keyboard or arcade encoder by its ordinary device name.
2. Select **Use this device**.
3. If the device does not identify itself as a keyboard, open **Other devices
   (optional)**. That list is deliberately separate from the normal path.

This choice is only a draft. Selecting a device does not disconnect it, create
a controller, or write anything to disk.

## 3. Choose a controller

Pick the virtual controller the keyboard should become, then add it. You can
change its type or remove it while the setup is still a draft.

ksx supports as many as 16 players. Xbox-style controllers occupy the four
places Windows games normally expose through XInput; supported additional
players use PlayStation-style controllers. The Setup screen shows the exact
capacity of the installed build instead of assuming four players.

Choose a ready-made controller layout that resembles the physical controls.
The default two-player keyboard layout includes:

- Player 1 Guide/Home: **Left Windows**
- Player 2 Guide/Home: **Numpad \***

## 4. Map the controls

Select **Map controls** on a player, or open **Controls**.

- Click a controller control, then press the physical key for it.
- Add more than one key to a control when useful.
- Configure chords, auto-fire, and macros in the same editor.
- Use the player tabs to move between controllers.

While editing this unsaved setup, changes remain in the background service's
draft. They do not write controller-layout files. A refused edit leaves the
draft unchanged.

## 5. Decide whether to split or freeze the keyboard

Return to **Setup** and answer the required question:

- **Split it:** only mapped keys become controller input; unused keys can still
  type or be assigned to another player.
- **Freeze it:** while Play is active, every other key on that keyboard is
  ignored. This prevents accidental typing during a game.

**LeftCtrl five times is always the escape hatch.** It stops keyboard capture
even if the app window is closed or unresponsive.

## 6. Save, Play, or both

These are separate actions:

- **Save this setup** keeps the keyboard, controllers, layouts, and split/freeze
  choice for later. It does not start Play.
- **Play now** uses exactly what is on the screen without saving it first.

When Play begins, the virtual controllers appear and the chosen keyboard starts
driving them. Open **Test** to see short and long presses light up.

Guide/Home opens Xbox Game Bar only when Game Bar is available and Windows'
**Allow your controller to open Game Bar** setting is enabled. Setup includes
an **Open Windows Game Bar settings** button. ksx never changes that Windows
setting silently.

## 7. Save a game (optional)

Open **Manage saved games** from the bottom of Setup when ksx should remember a
program or launcher link.

1. Enter a game name.
2. Paste the program path or launcher link. Surrounding quotation marks are
   accepted and removed safely.
3. Choose the player count and controller layout.
4. Select **Save game**.

Each player inherits the corresponding device choice from the saved Setup, so
the saved game is runnable rather than an empty controller shell. Existing
saved games can be switched, edited, rebased to the latest Setup device
choices, or deleted entirely in Studio. Controller layouts are kept when a
saved game is deleted.

## If Setup says the background service is unavailable

Close the ksx window and reopen **ksx** from the desktop or Start menu. If ksx
is already in the notification area, choose **Open Studio** there. No terminal
command is part of the customer recovery path.

The Setup and Saved Games screens keep raw device paths and read errors inside
**Technical details** or **Support details** disclosures. Include those details
when reporting a problem.

## What still requires physical release acceptance

Automated tests prove the installer contracts, no-console launcher, empty-setup
bootstrap, staged mapper, profile editing, and Guide bindings. They do not
prove a clean-machine install, a real controller in a real game, Windows Game
Bar activation, or the cabinet's long hardware soak. The exact supervised
release checklist and its unrun status are in [`GATES.md`](GATES.md), Gate 4.

Developers and maintainers should start with [`HANDOFF.md`](HANDOFF.md), then
use [`ARCHITECTURE.md`](ARCHITECTURE.md), [`SURFACES.md`](SURFACES.md), and the
developer CLI reference in the repository README.
