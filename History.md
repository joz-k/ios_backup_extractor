HISTORY of the iOS Backup Extractor
===================================

1.2.5   (2025-02-28)
--------------------

* Added `.gif` and `.webp` to the list of exported files.

    Files with these extensions are also exported from the camera roll.

* Enhanced text output during extraction.

    When extracting, the tool will now display the name of the internal "DCIM"
    folder where the media file was originally located in the device/backup
    filesystem.

* iCloud media extraction.

    Media files downloaded to the device from iCloud are also exported during
    the extraction process. To skip these files, the new `--ignore-icloud-media`
    option can be used.
  
1.2.4   (2024-11-13)
--------------------

* After export, media files have "Date Created" and "Date Modified" filesystem
  metadata attributes modified to reflect the dates originally set when created
  by iOS.

* Implemented `--prepend-date` and `--prepend-date-separator` options.

    These options make possible to prepend the creation date to each exported
    file. The files like `IMG_1111.heic` can be therefore 	exported as
    `2024-11-13_IMG_111.heic`. The date separator can be modified using
    `--prepend-date-separator` option.

* Implemented two special keywords for the `--since` parameter.

    Instead of DATE (YYYY-MM-DD), user can also enter one of these:
    - `last-week`, which means "current date" minus 8 days
    - `last-month`, which means "current date" minus 32 days

* Added export for Apple ProRAW format files

    Files with `.dng` extension (Apple ProRAW) added to the list of exported
    media files.


1.2.3   (2024-09-06)
--------------------

* The backup time is now displayed in a local timezone.

    Before this change, the `--list` report showed the backup time in
    a UTC timezone. This was confusing and unexpected. Now the backup time is
    always displayed in the local timezone.

* Added `.3gp` and `.mp4` to the list of exported files.

    Files with these extensions are also exported from the camera roll.
    Requested by #3.


1.2.2   (2024-07-14)
--------------------

* Added `--add-trash` option.

    With `-add-trash` option, it is now possible to extract also files marked
    as deleted. Such files contain "_DELETED" in their filename.

* Files are listed in a sorted order.

    Previously files were listed/copied in somehow random order. After this
    change, files should be listed/copied in a chronological order, from oldest
    to newest. This should make file listing more predictable because latest
    files should be at the end of the list.

* Fix for Manifest.plist parsing on Windows.

    Due to a bug in the `Mac::PropertyList` Perl module, binary plist files
    (Manifest.plist, Status.plist) are sometimes randomly appear to be
    corrupted when running on Windows. When this happens, the script usually
    ends with the infamous `Not a HASH reference at ios_backup_extractor.pl
    line 729` error. This change fixes this error.


1.2.1   (2024-03-22)
--------------------

* Added support for the new “Apple Devices” application on Windows.
* iOS backup version check. A stored backup version lower than 3.3 is rejected.


1.2.0   (2023-11-07)
--------------------

* The first public version.
* Provided Windows x64 binary is fully self-contained and requires Windows 7 or
  higher.

