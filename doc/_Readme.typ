// Create `Readme.pdf` using following command:
// typst compile _Readme.typ Readme.pdf

#let version_num = "1.2.5"

#import "@preview/note-me:0.4.0": *

// Set the page size to match the iPad Air 13-inch screen
#set page(width: 197.61mm, height: 263.27mm,
          numbering: "1")

#show link: set text(fill: blue)
#show link: underline


= iOS Backup Extractor v#version_num

`ios_backup_extractor` is a free command line utility to extract media files
(photos and videos) from a local #emph[unencrypted] iOS device backup. This
type of backup is typically created by Apple's
#link("https://apps.microsoft.com/detail/9pb2mz1zmb1s")[iTunes] or
"#link("https://apps.microsoft.com/detail/9np83lwlpz9k")[Apple Devices]"
application on Windows OS or the built-in Finder application on
MacOS#footnote[Follow instructions from Apple's
#link("https://support.apple.com/guide/iphone/back-up-iphone-iph3ecf67d29/ios")[iPhone
User Guide].].

== Download

Visit the #link("https://github.com/joz-k/ios_backup_extractor/releases")[Release Page].

For a full history of changes, see the
#link("https://github.com/joz-k/ios_backup_extractor/blob/main/History.md")[Change History].

#note[
Please refer to the "#link(<anchor_troubleshooting>)[Troubleshooting]" section
if the provided binaries display error messages.
]

== Screenshots

#image("res/win_screenshot1.png", alt: "screenshot1")
#image("res/macos_screenshot1.png", alt: "screenshot1")

== Motivation

This tool is designed to use unencrypted local iOS backups as a reliable and
incremental way to export photos and videos from iOS devices to your computer
without relying on any cloud services. Frequent (even daily) iOS backups are
very efficient because they are incremental (only new or changed files are
added to the backup). Additionally, the tool itself is fully incremental: it
never moves or overwrites files that have already been exported to the
specified output directory.

#strong[Note]: An unencrypted iOS backup won't contain most sensitive
information from your iOS device. For example, passwords, Wi-Fi settings,
website and call history, and health data are never
transferred#footnote[#link("https://support.apple.com/en-us/108353")]. So they
are not as insecure as you might think. However, it is important to back up
your iOS device data only to a fully secured computer.

== Troubleshooting <anchor_troubleshooting>

#strong[Problem:] I launched the command on MacOS and it shows the message
"`ios_backup_extractor` cannot be opened because the developer cannot be
verified."\
#strong[Solution:] You need to add this command to security exceptions
following these steps:
 1. In the Finder on your Mac, locate the app where it has been extracted
    (unzipped).
 2. Control-click the `ios_backup_extractor` icon, then choose Open from the
    shortcut menu.
 3. Click Open. This opens the Terminal window, launches the application and
    saves a security exception for it.
 4. You can now run the tool from the command line as usual.

#strong[Problem:] I run the tool on the MacOS and it says there are no iOS
backups available or I see an error message about directory access.\
#strong[Solution:] Go to Settings, `Security & Privacy` → `Full Disk Access`
and enable "Full Disk Access" for your `Terminal` application.
#pagebreak()

== `--help` screen

```
‘ios_backup_extractor’ extracts media files from an unencrypted
local backup of the iOS device made by iTunes or “Apple Devices” application
for Windows or by iPhone/iPad backup to MacOS computer.

Usage:
  ios_backup_extractor.exe [OPTIONS] DEVICE_SERIAL_ID | DEVICE_BACKUP_DIR --out OUTPUT_DIR
  ios_backup_extractor.exe [OPTIONS] --list

Commands:
  <default>           Extract media files from device backup.
  -l, --list          List available iOS device backups.
      --list-long     Like '--list' but prints more details.

Options:
  -f, --format FORMAT Determines a directory structure created in the output
                        directory. Valid values are:
                        - ‘ym’  for subdirectories like YYYY-MM (default)
                        - ‘ymd’ for subdirectories like YYYY-MM-DD
                        - ‘flat’ no subdirectories
  -s, --since DATE    Extract and copy only files created since DATE.
                        DATE must be in format YYYY-MM-DD or one
                        of the following special keywords:
                        - ‘last-week’
                        - ‘last-month’
      --add-trash     Extract also items marked as deleted.
      --prepend-date  Prepend a media creation date to each exported filename.
                        Default format is YYYY-MM-DD.
      --prepend-date-separator SEPARATOR
                      Change the separator for '--prepend-date' format.
                        Possible values are:
                        - ‘dash’ (default)
                        - ‘underscore’
                        - ‘none’
      --ignore-icloud-media
                      Do not extract media downloaded to the device from iCloud.
  -d, --dry           Dry run, don't copy any files.
  -v, --verbose       Show more information while running.
  -h, --help          Display help.

Examples:
  List all available iOS backups to determine device serial numbers.

      ios_backup_extractor.exe --list

  Extract all media files for a device with the serial number 'ABC123ABC123'
  to 'My Photos and Videos' directory. Such directory must exists already.

      ios_backup_extractor.exe ABC123ABC123 -o "My Photos and Videos"
```

== License

© 2025 #link("https://github.com/joz-k/")[joz-k],
#link("http://www.perlfoundation.org/artistic_license_2_0")[Artistic License
2.0]
