iOS Backup Extractor
====================

`ios_backup_extractor` is a command line utility to extract media files (photos and videos)
from a local _unencrypted_ iOS device backup. This type of backup is typically created by Apple's iTunes
or "Apple Devices" application on Windows OS or the built-in Finder application on MacOS[^1].

[^1]: Follow instructions from Apple's [iPhone User Guide](https://support.apple.com/guide/iphone/back-up-iphone-iph3ecf67d29/ios).

Download
---------

* Windows x64: [iOS_Backup_Extractor-v1.2.2_x64-windows.exe.zip](https://github.com/joz-k/ios_backup_extractor/releases/download/v1.2.2/iOS_Backup_Extractor-v1.2.2_x64-windows.exe.zip)
* MacOS arm64: [iOS_Backup_Extractor-v1.2.2_arm64-macos.zip](https://github.com/joz-k/ios_backup_extractor/releases/download/v1.2.2/iOS_Backup_Extractor-v1.2.2_arm64-macos.zip)
* MacOS intel64: [iOS_Backup_Extractor-v1.2.2_intel64-macos.zip](https://github.com/joz-k/ios_backup_extractor/releases/download/v1.2.2/iOS_Backup_Extractor-v1.2.2_intel64-macos.zip)
* Linux x64: [iOS_Backup_Extractor-v1.2.2_x64_linux.tar.gz](https://github.com/joz-k/ios_backup_extractor/releases/download/v1.2.2/iOS_Backup_Extractor-v1.2.2_x64_linux.tar.gz)

Screenshots
-----------

![screenshot1](doc/res/win_screenshot1.png "Windows screenhost")
![screenshot1](doc/res/macos_screenshot1.png "MacOS screenhost")

Troubleshooting
---------------

**Problem:** When I run this tool, I get the error message like `Not a HASH reference at ios_backup_extractor.pl line 729`.\
**Solution:** There was a bug in an earlier version of this tool, that randomly caused this type of error on Windows.
              Please use at least version 1.2.2 and this problem should be fixed.

**Problem:** I run the tool on the MacOS and it says there are no iOS backups available or I see an error message
             about directory access.\
**Solution:** Go to Settings, `Security & Privacy` → `Full Disk Access` and enable "Full Disk Access" for your
              `Terminal` application.

`--help` screen
---------------
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
                        DATE must be in format YYYY-MM-DD.
      --add-trash     Extract also items marked as deleted.
  -d, --dry           Dry run, don't copy any files.
  -v, --verbose       Show more information while running.
  -h, --help          Display help.

Examples:
  List all available iOS backups to determine device serial numbers.

      ios_backup_extractor.exe --list

  Extract all media files for a device with the serial number 'ABC123ABC123'
  to 'My Photos and Videos' directory. Such directory must exists already.

      ios_backup_extractor.exe ABC123ABC123 -o "My Photos and Videos"

Version:
    1.2.2 (2024-07-14)
```

License
-------

© 2024  [joz-k](https://github.com/joz-k/), [Artistic License 2.0](http://www.perlfoundation.org/artistic_license_2_0)
