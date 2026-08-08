<!-- The body of every ksx release. `.github/workflows/release.yml` substitutes
     VERSION, TAG, SETUP_NAME, SETUP_SHA256 and COMMIT (each written in double
     braces below) and publishes the result; a placeholder that workflow does
     not know fails the release rather than reaching the page as literal braces
     (crates/ksx-app/tests/installer.rs).

     ASCII only. This text is read, substituted and written back by PowerShell
     on a runner, and it is the first paragraph a stranger reads about ksx; a
     mojibake dash in that paragraph is a worse bug than it looks.

     It is prose in a reviewable file, and not a YAML string, because the
     paragraph about SmartScreen below is the one that decides whether a
     first-time user continues or stops. That paragraph deserves diffs. -->

**ksx {{VERSION}}** - Windows 11, 64-bit.

## Get it

Download **{{SETUP_NAME}}** from Assets below and double-click it. Click through
the wizard; at the end it offers to open ksx, and it leaves an icon on your
desktop either way.

## Windows will say "Windows protected your PC"

A blue box, with only a "Don't run" button showing. Click **More info**, then
**Run anyway**.

The honest reason: this installer is not code-signed. SmartScreen shows that box
for any installer whose publisher it does not recognise - it is a statement about
a certificate this project has not bought, not a finding about the file. If you
would rather check the file than take that sentence on faith, the SHA-256 below
is the one this release was built with, and it names the commit it was built
from.

## Verify it (optional)

Open PowerShell in your Downloads folder and run:

    Get-FileHash .\{{SETUP_NAME}} -Algorithm SHA256

It should print:

    {{SETUP_SHA256}}

Built from commit {{COMMIT}} by the `Release` workflow on a GitHub runner - no
developer machine touched these bytes.

## What is in Assets

- **{{SETUP_NAME}}** - the installer. This is the file.
- **ksx.exe** - the bare program, for people who want no installer. No Start
  menu entry, no desktop icon, and none of the bundled ViGEmBus driver that ksx
  expects to find in a `drivers` folder beside it.

Neither one installs a driver on its own. ksx installs ViGEmBus only when you
ask it to, and checks the bundle's hash and its signature first.
