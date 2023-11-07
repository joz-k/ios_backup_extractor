iOS Backup Extractor
====================

`ios_backup_extractor` is a command line utility to extract media files (phothos and videos)
from a local iOS device backup. This type of backup is typically created by Apple's iTunes
application on Windows OS or the built-in Finder application on MacOS[^1].

[^1]: Follow instructions from Apple's [iPhone User Guide](https://support.apple.com/guide/iphone/back-up-iphone-iph3ecf67d29/ios).

![screenshot1](doc/res/win_screenshot1.png "Windows screenhost")

`--help` screen
---------------
```
‘ios_backup_extractor.exe’ extracts media files from an unencrypted
local backup of the iOS device made by iTunes application for Windows
or by iPhone/iPad backup to MacOS computer.

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
    1.2.0 (2023-11-06)
```

Download
---------

* [iOS_Backup_Extractor-v1.2.0_x64-windows.exe.zip](https://github.com/joz-k/ios_backup_extractor/releases/download/v1.2.0/iOS_Backup_Extractor-v1.2.0_x64-windows.exe.zip)

Author
------

Jozef Kosoru, https://github.com/joz-k/


License
-------

© 2023 Jozef Kosoru, [Artistic License 2.0](http://www.perlfoundation.org/artistic_license_2_0)
