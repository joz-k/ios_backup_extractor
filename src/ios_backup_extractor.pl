#!perl

# ══════════════════════════════════════════════════════════════════════════ #
#                            iOS Backup Extractor                            #
#           ――――――――――――――――――――――――――――――――――――――――――――――――――――――           #
#            © 2024 Jozef Kosoru                      See LICENSE            #
# ══════════════════════════════════════════════════════════════════════════ #

use v5.38;
use warnings 'all';
use utf8;

use Data::Dumper   ();
use Getopt::Long   ();
use Scalar::Util   ();
use Time::Piece    ();
use File::Temp     ();
use File::Copy     ();
use File::Spec     ();
use File::Basename ();
use Digest::SHA    ();
use DBI            ();
use DBD::SQLite    ();
use DBD::SQLite::Constants qw[:file_open];
use Term::ReadKey     ();
use IO::Interactive   ();
use Mac::PropertyList ();
use Mac::PropertyList::ReadBinary ();

use constant {
    APP_NAME    => 'ios_backup_extractor',
    APP_VERSION => '1.2.4 (2024-11-13)',
};

my $wanted_extensions    = 'jpg|jpeg|heic|dng|png|mov|3gp|mp4|gif|webp';
my @format_options       = qw(ym ymd flat);
my %prepend_date_formats = ( 'dash'       => '%Y-%m-%d_',
                             'underscore' => '%Y_%m_%d_',
                             'none'       => '%Y%m%d_'  ,
                           );

my $g_backup_dir         = undef;
my $g_out_dir            = undef;
my @g_since_date         = (); # (YYYY, MM, DD)
my %g_deleted_files      = ();
my %g_manifest_plist_map = ();
my %g_status_plist_map   = ();

use constant {
    kSerialNumber => 0,
    kBackupDir    => 1,
    kError        => 2,
};

# define command line options
my %cmd_options;

Getopt::Long::GetOptions(
    \%cmd_options,
    'help|h',
    'list|l',       # list available device backups
    'format|f=s',   # output subdirectory format
    'since|s=s',    # since DATE
    'add-trash',    # extract also files from deleted
    'verbose|v',    # display more info and warnings
    'dry|d',        # “dry run”: doesn't make any real changes to the filesystem
    'out|o=s',      # output directory
    'list-long',    # more detailed device backup listing than --list
    'prepend-date', # prepend date to each filename
    'prepend-date-separator=s', # '-' (default), '_', None
    'debug',        # enable internal debug messages (warning: a huge stderr output)
) or exit 1; # EXIT_FAILURE;

main();

exit;

# -- end of the script code flow, subroutine definitions follow --
# ----------------------------------------------------------------

sub printHelp
{
    my $app_name = APP_NAME . getAppExtension();

    print <<~HELP_END;
        ‘$app_name’ extracts media files from an unencrypted
        local backup of the iOS device made by iTunes or “Apple Devices” application
        for Windows or by iPhone/iPad backup to MacOS computer.

        Usage:
          $app_name [OPTIONS] DEVICE_SERIAL_ID | DEVICE_BACKUP_DIR --out OUTPUT_DIR
          $app_name [OPTIONS] --list

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
          -d, --dry           Dry run, don't copy any files.
          -v, --verbose       Show more information while running.
          -h, --help          Display help.

        Examples:
          List all available iOS backups to determine device serial numbers.

              $app_name --list

          Extract all media files for a device with the serial number 'ABC123ABC123'
          to 'My Photos and Videos' directory. Such directory must exists already.

              $app_name ABC123ABC123 -o "My Photos and Videos"

        Version:
            ${ \APP_VERSION }

        Homepage:
            https://github.com/joz-k/ios_backup_extractor

        HELP_END
    exit;
}

# ----------------------------------------------------------------

sub main
{
    # verify command line arguments (and set defaults)
    my $show_help = checkAndSetArgs();

    # either there is a correct number of arguments or show help
    printHelp() if $show_help;

    # run choosen action
    if ($cmd_options{list})
    {
        listBackups();
    }
    else
    {
        extractMediaFiles();
    }
}

# ----------------------------------------------------------------

sub openDeviceBackupDir ($ios_backup_dir)
{
    -d $ios_backup_dir
        or die qq{Error: Unable to read directory:\n}
               . qq{    "$ios_backup_dir"\n}
               . qq{There are no iOS device backups on this computer.\n};

    say STDERR qq{Info: Found iOS Device backup directory: "$ios_backup_dir"}
        if $cmd_options{verbose};

    # list possible device backup directories
    opendir (my $dh, $ios_backup_dir) or die qq{Error: Cannot access "$ios_backup_dir": $!\n};

    return $dh;
}

# ----------------------------------------------------------------

sub listBackupDirs ($ios_backup_dir, $backup_dir_dh)
{
    return sort
           grep {    -d $_
                  && -f "$_/Info.plist"
                  && -f "$_/Manifest.plist"
                  && -f "$_/Status.plist"
                }
           map  { File::Spec->catfile ($ios_backup_dir, $_); }
           readdir ($backup_dir_dh);
}

# ----------------------------------------------------------------

sub enumerateBackups
{
    my @device_backup_dirs;

    for my $ios_backup_dir (getMobileSyncBackupDirs())
    {
        my $dh = openDeviceBackupDir ($ios_backup_dir);
        push @device_backup_dirs, listBackupDirs ($ios_backup_dir, $dh);
    }

    scalar @device_backup_dirs > 0
        or die "There are no iOS device backups found on this computer.\n";

    my %device_backup_map;

    my $wait_msg = "Reading list of iOS backups...";
    my $wait_msg_len = length ($wait_msg) + 1;
    my $clear_msg = "\b" x $wait_msg_len . q{ } x $wait_msg_len . "\b" x $wait_msg_len;

    print STDERR $wait_msg unless $cmd_options{verbose};

    for my $backup_dir (@device_backup_dirs)
    {
        say STDERR "Info: Reading directory: $backup_dir"
            if $cmd_options{verbose};

        my ($ok, %device_info, %manifest_plist, %status_plist);

        # read Info.plist
        ($ok, %device_info) = readInfoPlist ($backup_dir);

        $ok && $device_info{'Serial Number'} or do {
            warn $clear_msg, qq{Warning: Unable to read "$backup_dir/Info.plist".\n};
            next;
        };

        # read Manifest.plist
        ($ok, %manifest_plist) = readManifestPlist ($backup_dir);

        $ok or do {
            warn $clear_msg, qq{Warning: Unable to read "$backup_dir/Manifest.plist".\n};
            next;
        };

        # read Status.plist
        ($ok, %status_plist) = readStatusPlist ($backup_dir);

        $ok or do {
            warn $clear_msg, qq{Warning: Unable to read "$backup_dir/Status.plist".\n};
            next;
        };

        my $device_serial_number = $device_info{'Serial Number'};

        # add this backup to the map if newer is not already there
        if (   !exists ($device_backup_map{$device_serial_number})
            || compareBackupDates ($device_backup_map{$device_serial_number}
                                                        {Info}{'Last Backup Date'},
                                   $device_info{'Last Backup Date'})
           )
        {
            $device_backup_map{$device_serial_number}
                            = { Info     => \%device_info,
                                Manifest => \%manifest_plist,
                                Status   => \%status_plist,
                                Location => $backup_dir,
                              };
        }
    }

    scalar keys %device_backup_map > 0
        or die $clear_msg, "There are no usable iOS device backups found on this computer.\n";

    print STDERR $clear_msg; # clear "Reading..." message

    return %device_backup_map;
}

# ----------------------------------------------------------------

sub listBackups
{
    my %device_backup_map = enumerateBackups ();

    if ($cmd_options{'list-long'})
    {
        displayBackupListLong (\%device_backup_map);
    }
    else # `--list`
    {
        displayBackupList (\%device_backup_map);
    }
}

# ----------------------------------------------------------------

sub displayBackupList ($all_devices_backup_hashref)
{
    my ($serial, $name, $device, $backup_date, $encrypted);
    $= = 100; # increase the number of lines per page from 60 to 100

format STDOUT_TOP =

@||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||||
'Available iOS Device Backups'
--------------- --------------- -------------- ------------------ -----------
 Serial Number   Name            Device         Backup Date        Encrypted
--------------- --------------- -------------- ------------------ -----------
.
format STDOUT =
 @<<<<<<<<<<<<<  @<<<<<<<<<<...  @<<<<<<<<<...  @<<<<<<<<<<<<<<<<  @|||||||||
 $serial,        $name,          $device,       $backup_date,      $encrypted
.

    for $serial (sort keys %$all_devices_backup_hashref)
    {
        my $device_backup_info_hashref = $all_devices_backup_hashref->{$serial};
        $name   = $device_backup_info_hashref->{Info}{'Display Name'};
        $device = $device_backup_info_hashref->{Info}{'Product Name'},
        $backup_date = utcTimeToLocaltime (
                                $device_backup_info_hashref->{Info}{'Last Backup Date'});
        $encrypted = $device_backup_info_hashref->{Manifest}{IsEncrypted}
                   ? 'Yes'
                   : 'No';

        write;
    }

    print "\n";
}

# ----------------------------------------------------------------

sub utcTimeToLocaltime ($time)
{
    # $time is in format '2023-11-03T17:18:33Z'
    $time =~ /\A \d{4}-\d\d-\d\d T \d\d\:\d\d\:\d\d (?:\.\d+)? Z \z/x
        or return 'Unknown';

    my $utc_tp = Time::Piece->strptime ($time, '%FT%TZ')
        or return 'Unknown';

    my $local_tp = Time::Piece->localtime ($utc_tp->epoch);
    return $local_tp->strftime ('%Y-%m-%d %H:%M');
}

# ----------------------------------------------------------------

sub displayBackupListLong ($all_devices_backup_hashref)
{
    my $counter = 1;

    for my $device_serial (sort keys %$all_devices_backup_hashref)
    {
        my $device_backup_info_hashref = $all_devices_backup_hashref->{$device_serial};

        printf qq(%02d. %s, "%s", [%s]\n),
               $counter++,
               $device_serial,
               $device_backup_info_hashref->{Info}{'Display Name'},
               $device_backup_info_hashref->{Info}{'Product Name'},
               ;
        say q{-} x 78;

        printf q{ } x 4 . qq(Backup Dir: "%s"\n), $device_backup_info_hashref->{Location};

        my $printHashRefFunc = sub ($section) {
            my $plist_hashref = $device_backup_info_hashref->{$section};

            for my $key (sort keys %$plist_hashref)
            {
                printf q{ } x 4 . qq($section.plist/%s: %s\n),
                       $key,
                       $plist_hashref->{$key} // q{},
                       ;
            }
        };

        $printHashRefFunc->($_) for qw/Info Manifest Status/;
        print qq{\n};
    }
}

# ----------------------------------------------------------------

sub extractMediaFiles
{
    say STDERR q{Info: "--dry" mode enabled. No files will be copied.}
        if $cmd_options{dry};

    # abort if the device backup is encrypted
    die "Error: Device backup is ecnrypted. Encrypted backups are not supported.\n"
        if (isDeviceBackupEncrypted ($g_backup_dir));

    # check the backup version
    checkBackupVersion ($g_backup_dir);

    # create a temporary directory for `Manifest.db`
    my $tmp_fh = File::Temp->newdir ('iOS-Backup-Extractor-XXXXXXXXXX',
                                     TMPDIR => 1); # create in Temp dir instead of current dir

    say STDERR 'Info: Temp directory: ', $tmp_fh->dirname
        if ($cmd_options{verbose});

    # make the list of deleted files to skip (if requested)
    createDeletedFileList ($tmp_fh);

    # copy `Manifest.db` to the temporary directory
    my $orig_manifest_db = "$g_backup_dir/Manifest.db";
    my $tmp_manifest_db = qq[${\($tmp_fh->dirname)}/Manifest.db];

    File::Copy::copy ($orig_manifest_db, $tmp_manifest_db)
                                     or die "Error: File copy failed: $!\n";

    my $dbh = DBI->connect ("dbi:SQLite:dbname=$tmp_manifest_db",
                            undef,
                            undef,
                            {
                                sqlite_open_flags => SQLITE_OPEN_READONLY,
                                PrintError => 0,
                            })
        or die "Error: Cannot open ‘$tmp_manifest_db’ as SQLite db: $DBI::errstr.\n";

    my $eval_ok = eval {
        my $sql = <<~SQL_END;
            SELECT fileID, relativePath, file FROM files
                WHERE domain='CameraRollDomain'
                ORDER BY relativePath ASC
            SQL_END

        my $sth = $dbh->prepare ($sql)
            or die qq{Error: 'prepare' method failed on ‘$tmp_manifest_db’ SQLite db:\n},
                   qq{\t$DBI::errstr.\n};

        $sth->execute()
            or die qq{Error: 'execute' method failed on ‘$tmp_manifest_db’ SQLite db:\n},
                   qq{\t$DBI::errstr.\n};

        my $file_index = 1;

        # filter all full-size JPG, HEIC... images
        while (my $row = $sth->fetchrow_hashref)
        {
            my $file_id = $row->{fileID};
            my $relative_path = $row->{relativePath};

            next unless (   $relative_path !~ /thumb/i
                         && $relative_path !~ /metadata/i
                         && $relative_path =~ /\.$wanted_extensions$/i);

            # determine filename
            unless ($relative_path =~ m{^ .+
                                        /DCIM
                                        /\d+APPLE
                                        /(?<filename>[^./]+)(?:\.|/)
                                        .*
                                        (?<extension>
                                           (?i:$wanted_extensions)
                                        )
                                        $
                                       }x)
            {
                warn   qq{Warning: Cannot determine filename from "$relative_path"\n}
                     . qq{\tfileID: $file_id\n}
                    if ($cmd_options{verbose});

                next;
            }

            # add '_DELETED' flag to files marked as deleted
            my $deleted_flag = ($cmd_options{'add-trash'} && $g_deleted_files{$relative_path})
                             ? '_DELETED'
                             : q{};

            my $filename = $+{filename} . $deleted_flag . q{.} . lc $+{extension};

            # find the file in the blob storage
            my $subdir = $file_id =~ s/^(\w\w).+$/$1/r
                or die qq{Error: Unexpected fileID: $file_id\n};
            my $blob_file = "$g_backup_dir/$subdir/$file_id";
            -f $blob_file or die qq{Error: "$blob_file" doesn't exists\n};

            # parse the bplist from the SQLite database for this entry
            my $bplist_obj = parseBPlist ($row->{file}, $file_id);

            # find the "LastModified" date for this file
            my $lastmodif_time_piece
                            = defined $bplist_obj
                            ? getLastModifiedTimeFromBPListObj ($bplist_obj, $file_id)
                            : undef;

            # say STDERR "\tLastModified: ", $lastmodif_time_piece->strftime('%F %T');

            # find the "Birth" date for this file
            my $birth_time_piece
                            = defined $bplist_obj
                            ? getBirthTimeFromBPListObj ($bplist_obj, $file_id)
                            : undef;

            # say STDERR "\tBirth ", $birth_time_piece->strftime('%F %T');

            # skip if `--since` option is defined and this file is older than
            # specified DATE
            next if (   @g_since_date
                     && defined $lastmodif_time_piece
                     && olderThanSince ($lastmodif_time_piece, \@g_since_date));

            # determine output directory baseod on LastModified date
            my $date_sub_dir = getDateSubDir ($lastmodif_time_piece);
            my $out_sub_dir = $g_out_dir;
            $out_sub_dir .= "/$date_sub_dir" if $date_sub_dir ne q{};

            # create the output directory (if needed)
            mkdir ($out_sub_dir) unless (-d $out_sub_dir or $cmd_options{dry});

            # find a suitable filename (or `undef` if duplicate)
            my $unique_filename = findUniqueFilename ($blob_file,
                                                      $filename,
                                                      $out_sub_dir,
                                                      $lastmodif_time_piece);

            if (   not($cmd_options{'add-trash'})
                && $g_deleted_files{$relative_path})
            {
                printf "%3d. ($subdir/$file_id) %-13s → <IN_TRASH>, Skipping...\n",
                       $file_index, $filename;
            }
            elsif (not defined $unique_filename)
            {
                printf "%3d. ($subdir/$file_id) %-13s → <DUPLICATE>, Skipping...\n",
                       $file_index, $filename;
            }
            else
            {
                my $display_filename = $date_sub_dir ne q{}
                                     ? "$date_sub_dir/$unique_filename"
                                     : $unique_filename;

                printf "%3d. ($subdir/$file_id) %-13s → $display_filename\n",
                       $file_index, $filename;

                unless ($cmd_options{dry})
                {
                    # copy file to output directory
                    my $out_file = "$out_sub_dir/$unique_filename";
                    File::Copy::copy ($blob_file, $out_file)
                                               or die "Error: File copy failed: $!\n";

                    setFileAttributes ($out_file, $lastmodif_time_piece, $birth_time_piece);
                }
            }

            $file_index += 1;
        }

        1; # return success for eval
    };

    unless ($eval_ok)
    {
        my $error = $@;
        $dbh->disconnect;
        die $error;
    }

    $dbh->disconnect;
}

# ----------------------------------------------------------------

sub checkAndSetArgs
{
    # return true if command should just return a help screen
    return 1 if ($cmd_options{help});

    # `--list-long` is subset of `--list`
    $cmd_options{list} = 1 if $cmd_options{'list-long'};

    unless ($cmd_options{list})
    {
        # extract mode requires directory or deviceID argument, show help
        return 1 if (scalar @ARGV < 1);

        # read the serial_id/backup_directory arg
        my $backup_dir_or_serial = shift @ARGV;
        $backup_dir_or_serial =~ s/^\s+//;  # eat space left,
        $backup_dir_or_serial =~ s/\s+$//;  #           right

        my $kind;
        ($kind, $g_backup_dir) = resolveDirOrSerialArg ($backup_dir_or_serial);

        $kind != kError
            or die   qq{Error: ‘$backup_dir_or_serial’ doesn't look like a device serial },
                     qq{number or an iOS backup directory.\n};

        # output directory must be an existing writable directory
        defined $cmd_options{out} or die qq{Error: --out OUT_DIRECTORY option not specified.\n};
        $g_out_dir = $cmd_options{out};
        -d $g_out_dir or die qq{Error: ‘$g_out_dir’ is not a valid directory.\n};

        # there should be no addition arguments
        scalar @ARGV == 0
            or die qq(Error: Unknown argument: ${ \shift @ARGV }\n);
    }

    # format must be one of those from @format_options
    $cmd_options{format} //= 'ym'; # default
    grep { $_ eq lc $cmd_options{format} } @format_options
        or die qq{Error: --format must be one of the following: @format_options\n};

    # read the `since` date if defined
    if (defined (my $since_arg = $cmd_options{since}))
    {
        $since_arg =~ s/_/-/g;

        if (lc ($since_arg) eq 'last-week')
        {
            # now, minus 8 days
            my $since_last_week = (Time::Piece::localtime) - (8 * 24 * 60 * 60);
            $since_arg = $since_last_week->strftime('%Y-%m-%d');
            say STDERR qq{Info: Computed 'last-week' date: "$since_arg"}
                if $cmd_options{verbose};
        }
        elsif (lc ($since_arg) eq 'last-month')
        {
            # now, minus 32 days
            my $since_last_month = (Time::Piece::localtime) - (32 * 24 * 60 * 60);
            $since_arg = $since_last_month->strftime('%Y-%m-%d');
            say STDERR qq{Info: Computed 'last-month' date: "$since_arg"}
                if $cmd_options{verbose};
        }

        my ($date_ok, $year, $month, $day) = parseIsoDate ($since_arg);
        $date_ok or die qq{Error: Invalid --since DATE: "$since_arg"\n};

        @g_since_date = ($year, $month, $day);
    }

    # prepend date separator must be one of those from %prepend_date_formats
    $cmd_options{'prepend-date-separator'} //= 'dash';
    grep { $_ eq lc $cmd_options{'prepend-date-separator'} } keys %prepend_date_formats
        or die q{Error: --prepend-date-separator must be one of the following: }
               . join (', ', keys %prepend_date_formats) . qq{\n};

    # args OK, no need to show help screen
    return 0;
}

# ----------------------------------------------------------------

sub resolveDirOrSerialArg ($backup_dir_or_serial)
{
    # first try to resolve it as a device backup directory
    # such directory must exists and must contain `Manifest.db` file
    my $manifest_db_filename = "$backup_dir_or_serial/Manifest.db";
    my $backup_dir = $backup_dir_or_serial;

    return (kBackupDir, $backup_dir)
                if (-f $manifest_db_filename);

    return (kSerialNumber, $backup_dir)
                if ($backup_dir = findBackupDirForSerial ($backup_dir_or_serial));

    return (kError, undef);
}

# ----------------------------------------------------------------

sub findBackupDirForSerial ($device_serial)
{
    my %device_backup_map = enumerateBackups ();

    if (exists $device_backup_map{$device_serial})
    {
        my $backup_dir = $device_backup_map{$device_serial}{Location};

        say STDERR qq{Info: Using iOS Device backup directory: "$backup_dir"}
            if $cmd_options{verbose};

        return $backup_dir;
    }
    else
    {
        # no backup found for a given device serial number
        return undef;
    }
}

# ----------------------------------------------------------------

sub getMobileSyncBackupDirs
{
    my @mobile_sync_dirs;

    if ($^O =~ /mswin32/i)
    {
        # push the MobileSync path for newer "Apple Devices" application
        my $home_dir = $ENV{USERPROFILE};
        $home_dir ||= do {
            require Win32;
            Win32::GetFolderPath (Win32::CSIDL_PROFILE());
        };

        -d $home_dir or
            die "Error: Unable to determine %USERPROFILE% directory.\n";

        my $apple_devices_mobile_sync_backup_dir = File::Spec->catfile ($home_dir,
                                                                        'Apple',
                                                                        'MobileSync',
                                                                        'Backup');

        push @mobile_sync_dirs, $apple_devices_mobile_sync_backup_dir
            if -d $apple_devices_mobile_sync_backup_dir;

        # push the MobileSync path for iTunes application
        my $app_data_dir = $ENV{APPDATA};
        $app_data_dir ||= do {
            require Win32;
            Win32::GetFolderPath (Win32::CSIDL_APPDATA());
        };

        -d $app_data_dir or
            die "Error: Unable to determine %APPDATA% directory.\n";

        my $itunes_mobile_sync_backup_dir = File::Spec->catfile ($app_data_dir,
                                                                 'Apple Computer',
                                                                 'MobileSync',
                                                                 'Backup');

        push @mobile_sync_dirs, $itunes_mobile_sync_backup_dir
            if -d $itunes_mobile_sync_backup_dir;
    }
    elsif ($^O =~ /darwin/i)
    {
        push @mobile_sync_dirs, "$ENV{HOME}/Library/Application Support/MobileSync/Backup";
    }
    else
    {
        die   qq{Error: Unable to determine "Apple Computer/MobileSync/Backup" directory }
            . qq{on $^O platform.\n};
    }

    return @mobile_sync_dirs;
}

# ----------------------------------------------------------------

sub readInfoPlist ($device_backup_dir)
{
    my $plist = parsePList ("$device_backup_dir/Info.plist");

    return (0) unless $plist;

    my %info_list = readStringsFromPListDict ($plist,
        'Build Version' , 'Device Name'      , 'Display Name', 'GUID'             ,
        'ICCID'         , 'IMEI 2'           , 'IMEI'        , 'MEID'             ,
        'Phone Number'  , 'Product Name'     , 'Product Type', 'Product Version'  ,
        'Serial Number' , 'Target Identifier', 'Target Type' , 'Unique Identifier',
        'iTunes Version',
        );

    # add last backup date as a time value
    ($info_list{'Last Backup Date'}) = map  { $_->value; }
                                       grep { isOfPListType ($_, 'date'); }
                                       $plist->{'Last Backup Date'};


    return (1, %info_list);
}

# ----------------------------------------------------------------

sub compareBackupDates ($date1, $date2)
{
    # compare two backup dates and return true if `$date2` is newer.
    # "date" is in format '2023-11-03T17:18:33Z'

    my $date1_time_piece = Time::Piece->strptime ($date1, '%FT%TZ');
    my $date2_time_piece = Time::Piece->strptime ($date2, '%FT%TZ');

    return $date1_time_piece < $date2_time_piece;
}

# ----------------------------------------------------------------

sub readManifestPlist ($device_backup_dir)
{
    # return manifest right away if it was memoized already
    return (1, %{$g_manifest_plist_map{$device_backup_dir}})
        if (defined $g_manifest_plist_map{$device_backup_dir});

    my $plist = parseBinaryPList ("$device_backup_dir/Manifest.plist");

    return (0) unless $plist;

    my %manifest_plist = readStringsFromPListDict ($plist, 'Version', 'SystemDomainsVersion');

    # add boolean values
    my @boolean_keys = qw/IsEncrypted WasPasscodeSet/;
    @manifest_plist{@boolean_keys} = map { readPListBoolean ($_); }
                                     @{$plist}{@boolean_keys};

    # add date as a time value
    ($manifest_plist{Date}) = map  { $_->value; }
                              grep { isOfPListType ($_, 'date'); }
                              $plist->{Date};

    # add entries from the `Lockdown` section
    my %lockdown_entries = (
                             # defaule values
                             ( SerialNumber   => q{},
                               UniqueDeviceID => q{} ),

                             # real values
                             readStringsFromPListDict ($plist->{Lockdown},
                                         qw/BuildVersion   DeviceName   ProductType
                                            ProductVersion SerialNumber UniqueDeviceID/)
                           );

    $manifest_plist{"Lockdown/$_"} = $lockdown_entries{$_} for keys %lockdown_entries;

    # memoize manifest
    $g_manifest_plist_map{$device_backup_dir} = \%manifest_plist;

    return (1, %manifest_plist);
}

# ----------------------------------------------------------------

sub isDeviceBackupEncrypted ($device_backup_dir)
{
    # return if manifest was already read
    return $g_manifest_plist_map{$device_backup_dir}{IsEncrypted}
        if (exists $g_manifest_plist_map{$device_backup_dir});

    # read Manifest.plist
    my ($ok, %manifest_plist) = readManifestPlist ($device_backup_dir);

    $ok or die qq{Error: Unable to read "$device_backup_dir/Manifest.plist".\n};

    return    defined $manifest_plist{IsEncrypted}
           && $manifest_plist{IsEncrypted} == 1;
}

# ----------------------------------------------------------------

sub checkBackupVersion ($device_backup_dir)
{
    # read Status.plist to inspect "Version" field
    my ($ok, %status_plist) = readStatusPlist ($device_backup_dir);

    # read the version field
    my ($verMajor, $verMinor) = $status_plist{Version} =~ /^(\d+)\.(\d+)/
        or die qq{Error: Unable to determine backup version from Status.plist\n};

    if (   int($verMajor) < 3
        || (int($verMajor) == 3 && int($verMinor) < 3))
    {
        die qq{Error: This tool supports iOS backups since version 3.3.\n}
            . qq{Found version: $verMajor.$verMinor\n};
    }
}

# ----------------------------------------------------------------

sub readStatusPlist ($device_backup_dir)
{
    # return status.plist right away if it was memoized already
    return (1, %{$g_status_plist_map{$device_backup_dir}})
        if (defined $g_status_plist_map{$device_backup_dir});

    my $plist = parseBinaryPList ("$device_backup_dir/Status.plist");

    return (0) unless $plist;

    my %status_plist = readStringsFromPListDict ($plist,
                           qw/UUID BackupState Version SnapshotState/
                       );

    $status_plist{IsFullBackup} = readPListBoolean ($plist->{IsFullBackup});

    ($status_plist{Date}) = map  { $_->value; }
                            grep { isOfPListType ($_, 'date'); }
                            $plist->{Date};

    # memoize manifest
    $g_status_plist_map{$device_backup_dir} = \%status_plist;

    return (1, %status_plist);
}

# ----------------------------------------------------------------

sub isOfPListType ($plist_node, $type_str)
{
    return (   defined $plist_node
            && Scalar::Util::blessed ($plist_node) eq "Mac::PropertyList::$type_str"
            && $plist_node->type eq $type_str);
}

# ----------------------------------------------------------------

sub readStringsFromPListDict ($plist_node, @keys)
{
    # returns hash of key-values;

    return () unless (   isOfPListType ($plist_node, 'dict')
                      && defined $plist_node->value);

    my %keyValues;

    for my $key (@keys)
    {
        $keyValues{$key} = exists $plist_node->{$key}
                         ? readPListString ($plist_node->{$key})
                         : q{};
    }

    return %keyValues;
}

# ----------------------------------------------------------------

sub readPListString ($plist_node)
{
    if (   isOfPListType ($plist_node, 'string')
        && defined $plist_node->value)
    {
        return $plist_node->value;
    }
    else
    {
        return q{};
    }
}

# ----------------------------------------------------------------

sub readPListBoolean ($plist_node)
{
    isOfPListType ($plist_node, 'true')  and return 1;
    isOfPListType ($plist_node, 'false') and return 0;
    return undef;
}

# ----------------------------------------------------------------

sub parseIsoDate ($date_str)
{
    defined $date_str or return (0); # invalid ISO date

    my ($year, $month, $day) = ($date_str =~ m/^(\d{4})\-(\d\d?)\-(\d\d?)$/)
        or return (0); # invalid ISO date

    # check for a realistic date
    my $ok = (1969 < $year  < 3000 &&
                 0 < $month < 13   &&
                 0 < $day   <= daysPerMonth ($year, $month));

    $ok ? return (1, $year, int($month), int($day))
        : return (0);
}

# ----------------------------------------------------------------

sub daysPerMonth ($year, $month)
{
    my $is_leap_year = $year % 4 == 0 && ($year % 100 != 0 || $year % 400 == 0);

    return ($month == 2) ? 28 + int($is_leap_year)
                         : 30 + ($month + ($month >= 8 ? 1 : 0)) % 2;
}

# ----------------------------------------------------------------

sub olderThanSince ($file_modif_timepiece, $since_date_aref)
{
    use experimental qw(refaliasing declared_refs);
    my ($a, \@b) = ($file_modif_timepiece, \@$since_date_aref);

    return (   $a->year < $b[0]
            || (   $a->year == $b[0]
                && (   $a->mon < $b[1]
                    || (   $a->mon == $b[1]
                        && $a->mday < $b[2]))));
}

# ----------------------------------------------------------------

sub findUniqueFilename ($blob_file, $filename, $out_sub_dir, $lastmodif_time_piece)
{
    # check if the filename is unique. If there is a file with the same name,
    # check if it is a duplicate. If not, find a new unique name for this file.

    if ($cmd_options{'prepend-date'} && defined $lastmodif_time_piece)
    {
        $filename = $lastmodif_time_piece->strftime (
                            $prepend_date_formats{lc $cmd_options{'prepend-date-separator'}})
                  . $filename;
    }

    # file doesn't exist, return the same filename
    return $filename if (not -e "$out_sub_dir/$filename");

    # compute the checksum of the file in the blob storage
    my $blob_checksum = computeFileChecksum ($blob_file);

    my $new_filename_proposal = $filename;
    my ($base_name, $extension)
                    = $new_filename_proposal =~ m/^(.+)(\.[^\.]+$)/
                        or die qq{Error: Unexpected filename: $new_filename_proposal\n};
    my $index = 0;

    while (-e "$out_sub_dir/$new_filename_proposal")
    {
        # compute the checksum of the existing file (if any)
        my $existing_file_checkum
                        = computeFileChecksum ("$out_sub_dir/$new_filename_proposal");

        # the same file already exists, no need to copy anything
        return undef if ($blob_checksum eq $existing_file_checkum);

        # propose new name
        $new_filename_proposal = $base_name . qq{.$index} . $extension;
        $index += 1;
    }

    return $new_filename_proposal;
}

# ----------------------------------------------------------------

sub computeFileChecksum ($filepath)
{
    my $sha = Digest::SHA->new ('256');
    $sha->addfile ($filepath, 'b');
    return $sha->hexdigest;
}

# ----------------------------------------------------------------

sub parseBPlist ($bplist_file_info_blob, $file_id)
{
    unless ($bplist_file_info_blob)
    {
        warn qq{Warning: Missing file bplist for fileID: $file_id\n}
            if $cmd_options{verbose};

        return undef;
    }

    # parse bplist from the SQLite database entry for fileID
    my $plist = parseBinaryPList (\$bplist_file_info_blob);

    unless ($plist)
    {
        warn qq{Warning: Cannot parse bplist for fileID: $file_id\n}
            if $cmd_options{verbose};

        return undef;
    }

    my $bplist_obj = Mac::PropertyList::plist_as_perl ($plist);

    unless ($bplist_obj)
    {
        warn qq{Warning: Cannot convert bplist to obj for fileID: $file_id\n}
            if $cmd_options{verbose};

        return undef;
    }

    # print bplist as XML in debug mode
    print STDERR 'Debug: ', Mac::PropertyList::plist_as_string ($plist)
        if $cmd_options{debug};

    return $bplist_obj;
}

# ----------------------------------------------------------------

sub getTimestampFromBPListObj ($bplist_obj, $file_id, $kind)
{
    # verify expected structure layout
    unless (   ref $bplist_obj eq 'HASH'
            && ref $bplist_obj->{'$objects'} eq 'ARRAY'
            && scalar (@{$bplist_obj->{'$objects'}}) > 2
            && defined $bplist_obj->{'$objects'}[1]{$kind}
            && $bplist_obj->{'$objects'}[1]{$kind} =~ m/^\d+$/)
    {
        warn qq{Warning: Cannot get $kind time for fileID: $file_id\n}
            if $cmd_options{verbose};

        return undef;
    }

    # find the "LastModified/Birth" date for this file
    my $last_modified_timestamp = $bplist_obj->{'$objects'}[1]{$kind};
    return Time::Piece::localtime ($last_modified_timestamp);
}

# ----------------------------------------------------------------

sub getLastModifiedTimeFromBPListObj ($bplist_obj, $file_id)
{
    return getTimestampFromBPListObj ($bplist_obj, $file_id, 'LastModified');
}

# ----------------------------------------------------------------

sub getBirthTimeFromBPListObj ($bplist_obj, $file_id)
{
    return getTimestampFromBPListObj ($bplist_obj, $file_id, 'Birth');
}

# ----------------------------------------------------------------

sub setFileAttributes ($file, $last_tp, $birth_tp)
{
    (defined $last_tp && defined $birth_tp) or return;

    my $setUtime = sub {
        utime ($last_tp->epoch, $last_tp->epoch, $file);
    };

    if ($^O =~ /mswin32/i)
    {
        require Win32API::File::Time;
        Win32API::File::Time::SetFileTime ($file,
                                           $last_tp->epoch,
                                           $last_tp->epoch,
                                           $birth_tp->epoch);
    }
    elsif ($^O =~ /darwin/i && macOsHasSetFileCmd())
    {
        # MacOS with SetFile command installed
        # Note: SetFile command is deprecated and installed with the Command
        #       line developers tools
        my $birth_time_str = $birth_tp->strftime ('%m/%d/%Y %H:%M');
        my $modif_time_str = $last_tp->strftime ('%m/%d/%Y %H:%M');

        system ('/usr/bin/SetFile', '-d', $birth_time_str,
                                    '-m', $modif_time_str,
                                    $file) == 0
            or $setUtime->();
    }
    else
    {
        # fallback for MacOS/Linux using utime system call
        # on MacOS, utime also modifies 'Date Created' attribute, if 'Date Modified'
        # is older than original 'Date Created' (which should be always the case)
        $setUtime->();
    }
}

# ----------------------------------------------------------------

sub macOsHasSetFileCmd ()
{
    my $hasSetFileCmd = sub {
        my $out = `/usr/bin/SetFile 2>&1`;
        return $out && $out =~ /^\s*usage/si;
    };

    state $hasSetFile = $hasSetFileCmd->();
    return !!($hasSetFile);
}

# ----------------------------------------------------------------

sub getDateSubDir ($lastmodif_time_piece)
{
    if (lc $cmd_options{format} eq 'flat')
    {
        return q{};
    }
    elsif (defined $lastmodif_time_piece)
    {
        return   lc $cmd_options{format} eq 'ym'
               ? $lastmodif_time_piece->strftime ('%Y-%m')
               : $lastmodif_time_piece->ymd;
    }
    else
    {
        return 'Unknown_Date';
    }
}

# ----------------------------------------------------------------

sub sqliteTableExists ($dbh, $table_name, $db_filename)
{
    my $sql = <<~SQL_END;
        SELECT count(*) FROM sqlite_master
            WHERE type='table' AND name='$table_name';
        SQL_END

    my $rows = $dbh->selectall_arrayref ($sql, { Columns => [1] })
        or do {
            warn qq{Warning: Unable to read "sqlite_master" table from\n},
                 qq{\t"$db_filename" database file: $DBI::errstr\n}
                        if $cmd_options{verbose};

            return undef;
        };

    return $rows->[0][0];
}

# ----------------------------------------------------------------

sub createDeletedFileList ($tmp_fh)
{
    # Deleted media info is located in:
    #   SQLite: 12/12b144c0bd44f2b3dffd9186d3f9c05b917cee25
    #   Table:  ZASSET
    #   Column: Z_PK          (Primary Key)
    #   Column: ZTRASHEDSTATE (1 if deleted)
    #   Column: ZTRASHEDDATE  (not NULL if deleted)
    #   Column: ZDIRECTORY    (e.g. 'DCIM/103/APPLE')
    #   Column: ZFILENAME     (e.g. 'IMG_6600.JPG)
    my $photos_db = '12/12b144c0bd44f2b3dffd9186d3f9c05b917cee25';

    # copy `Photos.sqlite` to the temporary directory
    my $tmp_photos_db = qq[${\($tmp_fh->dirname)}/Photos.sqlite];

    unless (File::Copy::copy ("$g_backup_dir/$photos_db", $tmp_photos_db))
    {
        warn qq{Warning: Cannot copy or find Photos.sqlite database\n},
             qq{\tfileID: $photos_db\n}
                    if $cmd_options{verbose};

        return;
    }

    my $dbh = DBI->connect ("dbi:SQLite:dbname=$tmp_photos_db",
                            undef,
                            undef,
                            {
                                sqlite_open_flags => SQLITE_OPEN_READONLY,
                                PrintError => 0,
                            });

    unless ($dbh)
    {
        warn qq{Warning: Cannot open ‘$tmp_photos_db’ as SQLite db: $DBI::errstr.\n}
            if $cmd_options{verbose};

        return;
    }

    # check if the ZASSET table exists
    sqliteTableExists ($dbh, 'ZASSET', $tmp_photos_db)
        or return;

    # read the list of deleted media files
    my $sql = <<~SQL_END;
        SELECT ZDIRECTORY, ZFILENAME FROM ZASSET
            WHERE ZTRASHEDSTATE = 1;
        SQL_END

    my $sth = $dbh->prepare ($sql)
        or do {
            warn qq{Warning: 'prepare' method failed on ‘$tmp_photos_db’ SQLite db:\n},
                 qq{\t$DBI::errstr.\n}
                        if $cmd_options{verbose};

            return;
        };

    $sth->execute()
        or do {
            warn qq{Warning: 'execute' method failed on ‘$tmp_photos_db’ SQLite db:\n},
                 qq{\t$DBI::errstr.\n}
                        if $cmd_options{verbose};

            return;
        };

    while (my $row = $sth->fetchrow_hashref)
    {
        my $del_file_relpath = 'Media/' . $row->{ZDIRECTORY} . q{/} . $row->{ZFILENAME};
        $g_deleted_files{$del_file_relpath} = 1;

        say STDERR qq{Info: File marked as deleted: $del_file_relpath}
            if ($cmd_options{verbose});
    }

    $dbh->disconnect
        or do {
            warn qq{Warning: Cannot properly disconnect ‘$tmp_photos_db’ SQLite db:},
                 qq{\t$DBI::errstr.\n}
                        if $cmd_options{verbose};
        };
}

# ----------------------------------------------------------------

sub parsePList ($filename)
{
    my $plist;

    # first try to parse with an universal parser
    my $eval_ok = eval {
        # disable STDERR for plist parsing to not confuse a user with warning
        # messages
        local *STDERR;
        open STDERR, '>', File::Spec->devnull() or die "Error: Could not open STDERR: $!\n";

        $plist = Mac::PropertyList::parse_plist_file ($filename);
    };

    # try parse again directly with the binary parser if previous attempt failed
    $plist = parseBinaryPList ($filename) unless ($eval_ok);

    return $plist;
}

# ----------------------------------------------------------------

sub parseBinaryPList ($filename_or_data)
{
    my ($bplist_parser, $plist);

    my $eval_ok = eval {
        # disable STDERR for plist parsing to not confuse a user with warning
        # messages
        local *STDERR;
        open STDERR, '>', File::Spec->devnull() or die "Error: Could not open STDERR: $!\n";

        $bplist_parser = Mac::PropertyList::ReadBinary->new ($filename_or_data);
    };

    $plist = $bplist_parser->plist
        if (   $eval_ok
            && Scalar::Util::blessed ($bplist_parser) eq 'Mac::PropertyList::ReadBinary');

    return $plist;
}

# ----------------------------------------------------------------

sub getAppExtension
{
    # choose binary extension
    return   $ENV{PAR_0}          # compiled to 'PAR' binary?
           ? (  $^O =~ /mswin32/i # on Windows?
              ? '.exe'            # binary on Windows     → '.exe'
              : q{} )             # binary on Linux/MacOS → no extension
           : '.pl';               # running as a script   → '.pl'
}

# ----------------------------------------------------------------

sub getTerminalWidth
{
    my $hasTputCmd = sub {
        my $out = `tput cols 2>&1`;
        return $out && $out =~ /^\d+\s*$/s;
    };

    local *STDERR;
    open STDERR, '>', File::Spec->devnull();

    return    (Term::ReadKey::GetTerminalSize)[0]
           || (   $hasTputCmd->()
               && int(`tput cols` =~ s/^\d+\K.*/$1/sr))
           || 80;
}

# ----------------------------------------------------------------

BEGIN {
    my $encoding = ':encoding(UTF-8)';

    if ($^O =~ /mswin32/i)
    {
        # BTW, for older Windows console to support ANSI control sequences, you need to set
        # in registry HKCU\Console\VirtualTerminalLevel to 1

        # This part sets the Windows console (including Mintty/Cygwin) to UTF-8 mode
        # if it wasn't set already
        require Win32::Console;
        Win32::Console::OutputCP (65001);

        # non-msys (Strawberry|Active) Perl requires a special treatment to properly display
        # UTF-8 in the console.
        $encoding = ':unix:utf8';
    }

    # allow utf-8 in the console output
    binmode STDOUT, $encoding;
    binmode STDERR, $encoding;
}

__END__
# Notes:
#
# Apple iOS backups are located:
# • On Windows:
#       (iTunes)        %APPDATA%\Apple Computer\MobileSync\Backup\<phone Id>
#       (Apple Devices) %USERPROFILE%\Apple\MobileSync\Backup\<phone Id>
# • On MacOS:   ~/Library/Application Support/MobileSync/Backup/<phone Id>
#
# Special backup files:
# • Info.plist                               : Device Info
# • Manifest.plist                           : Backup Info
# • Status.plist                             : Backup Status Info
# • Manifest.db                              : File Index
# • Manifest.mbdb                            : Old Backup format
# • 12b144c0bd44f2b3dffd9186d3f9c05b917cee25 : Media/PhotoData/Photos.sqlite, CameraRollDomain
# • 5a4935c78a5255723f707230a451d79c540d2741 : Call History
# • 2b2b0084a1bc3a5ac8c27afdf14afb42c61a19ca : Call History
# • 3d0d7e5fb2ce288813306e4d4636395e047a3d28 : Messages
# • 31bb7ba8914766d4ba40d6dfb6113c8b614be442 : Contacts/Address Book
# • cd6702cea29fe89cf280a76794405adb17f9a0ee : Contact Image data
# • b97b0c3bc8a6bb221d0849b450fbd92b5d06a301 : Home Screen Wallpaper
# • 86736007d0166a18c646c567279b75093fc066fe : Lock Screen Wallpaper
# • 2041457d5fe04d39d0ab481178355df6781e6858 : Calendar
# • ca3bc056d4da0bbf88b5fb3be254f3b7147e639c : Old Notes
# • 4f98687d8ab0d6d1a371110e6b7300f6e465bef2 : Notes
# • 4096c9ec676f2847dc283405900e284a7c815836 : Locations
# • b03b6432c8e753323429e15bc9ec0a8040763424 : iPhoto Backup
# • ade0340f576ee14793c607073bd7e8e409af07a8 : Known WiFi Networks
# • e74113c185fd8297e140cfcf9c99436c5cc06b57 : Web History
# • 992df473bbb9e132f4b3b6e4d33f72171e97bc7a : Voice Mails
# • 1fa8656eab4eef2f4f6388aea16cc5389bb78123 : Books, table ZBKLIBRARYASSET

