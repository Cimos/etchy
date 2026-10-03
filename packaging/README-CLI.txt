etchy command-line tool
=======================

This download is the etchy COMMAND-LINE tool. It has no window of its own:
double-clicking etchy.exe flashes a console and closes. Run it from a terminal
(PowerShell, cmd, or a shell on macOS/Linux) instead.

Looking for the desktop viewer? Download the etchy installer for your system
(.msi on Windows, .dmg on macOS, .deb or .AppImage on Linux) from:
  https://github.com/Cimos/etchy/releases


Quick start
-----------
Windows (PowerShell), from this folder:

  .\etchy.exe --version
  .\etchy.exe --format summary old-gerbers\ new-gerbers\
  .\etchy.exe old-gerbers\ new-gerbers\ --html diff.html

macOS / Linux:

  ./etchy --version
  ./etchy --format summary old-gerbers/ new-gerbers/
  ./etchy old-gerbers/ new-gerbers/ --html diff.html

Open diff.html in a browser to see the layer-by-layer report.
Two .pdf files instead of two folders gives a page-by-page schematic diff.

Exit codes (for CI): 0 = no differences, 1 = differences found, 2 = error.

Run `etchy --help` for every option. Full docs: https://cimos.github.io/etchy/docs.html


Putting etchy on your PATH (optional)
-------------------------------------
Windows: copy etchy.exe to a folder such as %USERPROFILE%\bin and add that
folder to PATH (Settings > System > About > Advanced system settings >
Environment Variables).

macOS / Linux: sudo mv etchy /usr/local/bin/

macOS may block the binary the first time ("cannot be opened because the
developer cannot be verified"). Allow it once with:
  xattr -d com.apple.quarantine ./etchy
