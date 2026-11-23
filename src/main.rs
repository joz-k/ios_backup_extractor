// ══════════════════════════════════════════════════════════════════════════ //
//                            iOS Backup Extractor                            //
//           ――――――――――――――――――――――――――――――――――――――――――――――――――――――           //
//            © 2026 Jozef Kosoru                      See LICENSE            //
// ══════════════════════════════════════════════════════════════════════════ //
//
// Rust port of src/ios_backup_extractor.pl (v1.2.6).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use clap::Parser;
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

const APP_VERSION: &str = "1.2.6 (unreleased)";

const WANTED_EXTENSIONS: &str = "jpg|jpeg|heic|dng|png|mov|3gp|mp4|gif|webp";

static EXT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)\.(?:{WANTED_EXTENSIONS})$")).unwrap());

// [DCIM]
// * Media/DCIM/101APPLE/IMG_1111.JPG
// * Media/PhotoData/Mutations/DCIM/101APPLE/IMG_1111/Adjustments/FullSizeRender.jpg
static DCIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^
        (?<media_location>.+/DCIM/)
        (?<media_subdir>\d+APPLE/)
        (?<filename>[^./]+)
        (?:\.|/)
        .*
        (?<extension>(?i:{WANTED_EXTENSIONS}))
        $"
    ))
    .unwrap()
});

// [iCloud]
// * Media/PhotoData/CPLAssets/group101/1A1A1A1A-1A1A-1A1A-1A1A-1A1A1A1A1A1A.HEIC
// * Media/PhotoData/Mutations/PhotoData/CPLAssets/group101/UUID/Adjustments/FullSizeRender.jpg
static ICLD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)^
        (?<media_location>.+/PhotoData/CPLAssets/)
        (?<media_subdir>group\d+/)
        (?<filename>[^./]+)
        (?:\.|/)
        .*
        (?<extension>(?i:{WANTED_EXTENSIONS}))
        $"
    ))
    .unwrap()
});

static RESERVED_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?$").unwrap());

static SINCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{1,2})-(\d{1,2})$").unwrap());

static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)").unwrap());

const INFO_KEYS: &[&str] = &[
    "Build Version",
    "Device Name",
    "Display Name",
    "GUID",
    "ICCID",
    "IMEI 2",
    "IMEI",
    "MEID",
    "Phone Number",
    "Product Name",
    "Product Type",
    "Product Version",
    "Serial Number",
    "Target Identifier",
    "Target Type",
    "Unique Identifier",
    "iTunes Version",
];

const LOCKDOWN_KEYS: &[&str] = &[
    "BuildVersion",
    "DeviceName",
    "ProductType",
    "ProductVersion",
    "SerialNumber",
    "UniqueDeviceID",
];

const PHOTOS_SQLITE_FILEID: &str = "12/12b144c0bd44f2b3dffd9186d3f9c05b917cee25";
const SMS_DB_FILEID: &str = "3d/3d0d7e5fb2ce288813306e4d4636395e047a3d28";

// ----------------------------------------------------------------
// Command line options

#[derive(Parser)]
#[command(
    name = "ios_backup_extractor",
    infer_long_args = true,
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Cli {
    /// Display help
    #[arg(short = 'h', long)]
    help: bool,

    /// List available iOS device backups
    #[arg(short = 'l', long)]
    list: bool,

    /// More detailed device backup listing than --list
    #[arg(long)]
    list_long: bool,

    /// Output subdirectory format: ym, ymd, flat
    #[arg(short = 'f', long)]
    format: Option<String>,

    /// Extract only files created since DATE
    #[arg(short = 's', long)]
    since: Option<String>,

    /// Extract also items marked as deleted
    #[arg(long)]
    add_trash: bool,

    /// Show more information while running
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Dry run, don't copy any files
    #[arg(short = 'd', long)]
    dry: bool,

    /// Output directory
    #[arg(short = 'o', long)]
    out: Option<String>,

    /// Prepend a media creation date to each exported filename
    #[arg(long)]
    prepend_date: bool,

    /// Separator for --prepend-date: dash, underscore, none
    #[arg(long)]
    prepend_date_separator: Option<String>,

    /// Extract also media attached to messages
    #[arg(short = 'm', long)]
    add_message_media: bool,

    /// Do not create user album folders
    #[arg(long)]
    ignore_album_folders: bool,

    /// Do not extract media downloaded to the device from iCloud
    #[arg(long)]
    ignore_icloud_media: bool,

    /// Enable internal debug messages (warning: a huge stderr output)
    #[arg(long)]
    debug: bool,

    /// Device serial ID or device backup directory
    target: Option<String>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Format {
    #[default]
    Ym,
    Ymd,
    Flat,
}

#[derive(Clone, Copy, Default)]
enum DateSep {
    #[default]
    Dash,
    Underscore,
    None,
}

impl DateSep {
    fn strftime(self) -> &'static str {
        match self {
            DateSep::Dash => "%Y-%m-%d_",
            DateSep::Underscore => "%Y_%m_%d_",
            DateSep::None => "%Y%m%d_",
        }
    }
}

#[derive(Default)]
struct Options {
    list_long: bool,
    format: Format,
    since: Option<NaiveDate>,
    add_trash: bool,
    verbose: bool,
    dry: bool,
    out_dir: PathBuf,
    prepend_date: bool,
    date_sep: DateSep,
    add_message_media: bool,
    ignore_album_folders: bool,
    ignore_icloud_media: bool,
    debug: bool,
}

#[derive(Default)]
struct AuxData {
    deleted: HashSet<String>,
    albums: HashMap<String, String>,
}

struct DeviceInfo {
    serial: String,
    display_name: String,
    product_name: String,
    last_backup: Option<DateTime<Utc>>,
    fields: BTreeMap<String, String>,
}

struct ManifestInfo {
    is_encrypted: bool,
    fields: BTreeMap<String, String>,
}

struct StatusInfo {
    version: String,
    fields: BTreeMap<String, String>,
}

struct BackupEntry {
    info: DeviceInfo,
    manifest: ManifestInfo,
    status: StatusInfo,
    location: PathBuf,
}

struct Backup {
    dir: PathBuf,
    manifest: ManifestInfo,
    status: StatusInfo,
}

// ----------------------------------------------------------------

fn main() {
    console_init();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = run(cli) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

// ----------------------------------------------------------------

fn run(cli: Cli) -> Result<()> {
    if cli.help {
        print_help();
        return Ok(());
    }

    let list = cli.list || cli.list_long;

    let mut opts = Options {
        list_long: cli.list_long,
        add_trash: cli.add_trash,
        verbose: cli.verbose,
        dry: cli.dry,
        prepend_date: cli.prepend_date,
        add_message_media: cli.add_message_media,
        ignore_album_folders: cli.ignore_album_folders,
        ignore_icloud_media: cli.ignore_icloud_media,
        debug: cli.debug,
        ..Options::default()
    };

    if list {
        validate_common(&cli, &mut opts)?;
        let backups = enumerate_backups(&opts)?;
        if opts.list_long {
            display_backup_list_long(&backups);
        } else {
            display_backup_list(&backups, terminal_width());
        }
        return Ok(());
    }

    // extract mode requires a directory or device serial argument
    let Some(ref target) = cli.target else {
        print_help();
        return Ok(());
    };
    let target = target.trim();

    let backup = resolve_backup(&opts, target)?;

    let Some(ref out) = cli.out else {
        bail!("Error: --out OUT_DIRECTORY option not specified.");
    };
    if !Path::new(&out).is_dir() {
        bail!("Error: ‘{out}’ is not a valid directory.");
    }
    opts.out_dir = PathBuf::from(out);

    validate_common(&cli, &mut opts)?;

    extract_media_files(&opts, &backup)
}

// ----------------------------------------------------------------

fn validate_common(cli: &Cli, opts: &mut Options) -> Result<()> {
    let format = cli.format.as_deref().unwrap_or("ym").to_lowercase();
    opts.format = match format.as_str() {
        "ym" => Format::Ym,
        "ymd" => Format::Ymd,
        "flat" => Format::Flat,
        _ => bail!("Error: --format must be one of the following: ym ymd flat"),
    };

    opts.since = parse_since(cli.since.as_deref(), opts.verbose)?;

    let sep = cli
        .prepend_date_separator
        .as_deref()
        .unwrap_or("dash")
        .to_lowercase();
    opts.date_sep = match sep.as_str() {
        "dash" => DateSep::Dash,
        "underscore" => DateSep::Underscore,
        "none" => DateSep::None,
        _ => bail!(
            "Error: --prepend-date-separator must be one of the following: dash, underscore, none"
        ),
    };

    Ok(())
}

// ----------------------------------------------------------------

fn print_help() {
    let app_name = app_name();
    print!(
        r#"‘{app_name}’ extracts media files from an unencrypted
local backup of the iOS device made by iTunes or “Apple Devices” application
for Windows or by iPhone/iPad backup to MacOS computer.

Usage:
  {app_name} [OPTIONS] DEVICE_SERIAL_ID | DEVICE_BACKUP_DIR --out OUTPUT_DIR
  {app_name} [OPTIONS] --list

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
  -m, --add-message-media
                      Extract also media attached to messages.
      --prepend-date  Prepend a media creation date to each exported filename.
                        Default format is YYYY-MM-DD.
      --prepend-date-separator SEPARATOR
                      Change the separator for '--prepend-date' format.
                        Possible values are:
                        - ‘dash’ (default)
                        - ‘underscore’
                        - ‘none’
      --ignore-album-folders
                      Do not create user album folders.
      --ignore-icloud-media
                      Do not extract media downloaded to the device from iCloud.
  -d, --dry           Dry run, don't copy any files.
  -v, --verbose       Show more information while running.
  -h, --help          Display help.

Examples:
  List all available iOS backups to determine device serial numbers.

      {app_name} --list

  Extract all media files for a device with the serial number 'ABC123ABC123'
  to 'My Photos and Videos' directory. Such directory must exists already.

      {app_name} ABC123ABC123 -o "My Photos and Videos"

Version:
    {APP_VERSION}

Homepage:
    https://github.com/joz-k/ios_backup_extractor

"#
    );
}

// ----------------------------------------------------------------
// Verbose logging helpers (warnings/info gated like the Perl original)

fn vwarn(opts: &Options, msg: &str) {
    if opts.verbose {
        eprintln!("{msg}");
    }
}

fn vinfo(opts: &Options, msg: &str) {
    if opts.verbose {
        eprintln!("{msg}");
    }
}

// ----------------------------------------------------------------
// Backup discovery & enumeration

fn mobile_sync_backup_dirs() -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();

    if cfg!(windows) {
        // MobileSync path for the newer "Apple Devices" application
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        let Some(home) = home.filter(|p| p.is_dir()) else {
            bail!("Error: Unable to determine %USERPROFILE% directory.");
        };
        let apple_devices = home.join("Apple").join("MobileSync").join("Backup");
        if apple_devices.is_dir() {
            dirs.push(apple_devices);
        }

        // MobileSync path for the iTunes application
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(dirs::config_dir);
        let Some(appdata) = appdata.filter(|p| p.is_dir()) else {
            bail!("Error: Unable to determine %APPDATA% directory.");
        };
        let itunes = appdata
            .join("Apple Computer")
            .join("MobileSync")
            .join("Backup");
        if itunes.is_dir() {
            dirs.push(itunes);
        }
    } else if cfg!(target_os = "macos") {
        let home = dirs::home_dir().context("Error: Unable to determine HOME directory.")?;
        dirs.push(home.join("Library/Application Support/MobileSync/Backup"));
    } else {
        bail!(
            concat!("Error: Unable to determine \"Apple Computer/MobileSync/Backup\" directory ",
                    "on {} platform."),
            std::env::consts::OS
        );
    }

    Ok(dirs)
}

fn enumerate_backups(opts: &Options) -> Result<BTreeMap<String, BackupEntry>> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    for root in mobile_sync_backup_dirs()? {
        if !root.is_dir() {
            bail!(
                concat!("Error: Unable to read directory:\n    \"{}\"\n",
                        "There are no iOS device backups on this computer."),
                root.display()
            );
        }
        vinfo(
            opts,
            &format!(
                "Info: Found iOS Device backup directory: \"{}\"",
                root.display()
            ),
        );

        let rd = fs::read_dir(&root)
            .map_err(|e| anyhow!("Error: Cannot access \"{}\": {e}", root.display()))?;
        let mut subdirs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.join("Info.plist").is_file()
                    && p.join("Manifest.plist").is_file()
                    && p.join("Status.plist").is_file()
            })
            .collect();
        subdirs.sort();
        candidates.extend(subdirs);
    }

    if candidates.is_empty() {
        bail!("There are no iOS device backups found on this computer.");
    }

    // progress message (only on an interactive terminal, erased afterwards)
    let clear_msg = if !opts.verbose && is_terminal() {
        let wait_msg = "Reading list of iOS backups...";
        eprint!("{wait_msg}");
        let _ = io::stderr().flush();
        let n = wait_msg.len() + 1;
        format!("{}{}{}", "\u{8}".repeat(n), " ".repeat(n), "\u{8}".repeat(n))
    } else {
        String::new()
    };

    let mut map: BTreeMap<String, BackupEntry> = BTreeMap::new();

    for dir in &candidates {
        vinfo(
            opts,
            &format!("Info: Reading directory: {}", dir.display()),
        );

        let Some(info) = read_info_plist(dir) else {
            eprintln!(
                "{clear_msg}Warning: Unable to read \"{}/Info.plist\".",
                dir.display()
            );
            continue;
        };
        if info.serial.is_empty() {
            eprintln!(
                "{clear_msg}Warning: Unable to read \"{}/Info.plist\".",
                dir.display()
            );
            continue;
        }
        let Some(manifest) = read_manifest_plist(dir) else {
            eprintln!(
                "{clear_msg}Warning: Unable to read \"{}/Manifest.plist\".",
                dir.display()
            );
            continue;
        };
        let Some(status) = read_status_plist(dir) else {
            eprintln!(
                "{clear_msg}Warning: Unable to read \"{}/Status.plist\".",
                dir.display()
            );
            continue;
        };

        // keep the newest backup per serial number
        let replace = match map.get(&info.serial) {
            None => true,
            Some(existing) => match (existing.info.last_backup, info.last_backup) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(old), Some(new)) => new > old,
            },
        };
        if replace {
            map.insert(
                info.serial.clone(),
                BackupEntry {
                    info,
                    manifest,
                    status,
                    location: dir.clone(),
                },
            );
        }
    }

    if map.is_empty() {
        bail!("{clear_msg}There are no usable iOS device backups found on this computer.");
    }

    if !clear_msg.is_empty() {
        eprint!("{clear_msg}");
    }

    Ok(map)
}

// ----------------------------------------------------------------

fn resolve_backup(opts: &Options, arg: &str) -> Result<Backup> {
    let as_dir = Path::new(arg);

    // first try to resolve the argument as a device backup directory
    if as_dir.join("Manifest.db").is_file() {
        let Some(manifest) = read_manifest_plist(as_dir) else {
            bail!(
                "Error: Unable to read \"{}/Manifest.plist\".",
                as_dir.display()
            );
        };
        let status = read_status_plist(as_dir).unwrap_or(StatusInfo {
            version: String::new(),
            fields: BTreeMap::new(),
        });
        return Ok(Backup {
            dir: as_dir.to_path_buf(),
            manifest,
            status,
        });
    }

    // then try to resolve it as a device serial number
    let backups = enumerate_backups(opts)?;
    if let Some(entry) = backups.get(arg) {
        vinfo(
            opts,
            &format!(
                "Info: Using iOS Device backup directory: \"{}\"",
                entry.location.display()
            ),
        );
        return Ok(Backup {
            dir: entry.location.clone(),
            manifest: ManifestInfo {
                is_encrypted: entry.manifest.is_encrypted,
                fields: entry.manifest.fields.clone(),
            },
            status: StatusInfo {
                version: entry.status.version.clone(),
                fields: entry.status.fields.clone(),
            },
        });
    }

    bail!("Error: ‘{arg}’ doesn't look like a device serial number or an iOS backup directory.")
}

// ----------------------------------------------------------------
// Plist reading

fn plist_string(dict: &plist::Dictionary, key: &str) -> String {
    dict.get(key)
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string()
}

fn plist_bool_display(dict: &plist::Dictionary, key: &str) -> String {
    match dict.get(key).and_then(|v| v.as_boolean()) {
        Some(true) => "1".to_string(),
        Some(false) => "0".to_string(),
        None => String::new(),
    }
}

fn plist_date(dict: &plist::Dictionary, key: &str) -> Option<DateTime<Utc>> {
    let date = dict.get(key)?.as_date()?;
    Some(DateTime::<Utc>::from(SystemTime::from(date)))
}

fn plist_date_display(dict: &plist::Dictionary, key: &str) -> String {
    plist_date(dict, key)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

fn read_info_plist(dir: &Path) -> Option<DeviceInfo> {
    let value = plist::Value::from_file(dir.join("Info.plist")).ok()?;
    let dict = value.as_dictionary()?;

    let mut fields = BTreeMap::new();
    for key in INFO_KEYS {
        fields.insert(key.to_string(), plist_string(dict, key));
    }
    let last_backup = plist_date(dict, "Last Backup Date");
    fields.insert(
        "Last Backup Date".to_string(),
        plist_date_display(dict, "Last Backup Date"),
    );

    Some(DeviceInfo {
        serial: plist_string(dict, "Serial Number"),
        display_name: plist_string(dict, "Display Name"),
        product_name: plist_string(dict, "Product Name"),
        last_backup,
        fields,
    })
}

fn read_manifest_plist(dir: &Path) -> Option<ManifestInfo> {
    let value = plist::Value::from_file(dir.join("Manifest.plist")).ok()?;
    let dict = value.as_dictionary()?;

    let mut fields = BTreeMap::new();
    for key in ["Version", "SystemDomainsVersion"] {
        fields.insert(key.to_string(), plist_string(dict, key));
    }
    for key in ["IsEncrypted", "WasPasscodeSet"] {
        fields.insert(key.to_string(), plist_bool_display(dict, key));
    }
    fields.insert("Date".to_string(), plist_date_display(dict, "Date"));

    if let Some(lockdown) = dict.get("Lockdown").and_then(|v| v.as_dictionary()) {
        for key in LOCKDOWN_KEYS {
            fields.insert(format!("Lockdown/{key}"), plist_string(lockdown, key));
        }
    } else {
        for key in LOCKDOWN_KEYS {
            fields.insert(format!("Lockdown/{key}"), String::new());
        }
    }

    Some(ManifestInfo {
        is_encrypted: dict.get("IsEncrypted").and_then(|v| v.as_boolean()) == Some(true),
        fields,
    })
}

fn read_status_plist(dir: &Path) -> Option<StatusInfo> {
    let value = plist::Value::from_file(dir.join("Status.plist")).ok()?;
    let dict = value.as_dictionary()?;

    let mut fields = BTreeMap::new();
    for key in ["UUID", "BackupState", "Version", "SnapshotState"] {
        fields.insert(key.to_string(), plist_string(dict, key));
    }
    fields.insert(
        "IsFullBackup".to_string(),
        plist_bool_display(dict, "IsFullBackup"),
    );
    fields.insert("Date".to_string(), plist_date_display(dict, "Date"));

    Some(StatusInfo {
        version: plist_string(dict, "Version"),
        fields,
    })
}

// ----------------------------------------------------------------
// Backup listing

fn utc_to_local(time: Option<DateTime<Utc>>) -> String {
    match time {
        Some(t) => t
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => "Unknown".to_string(),
    }
}

fn display_backup_list(backups: &BTreeMap<String, BackupEntry>, term_width: usize) {
    const MIN_WIDTH: usize = 68;
    const MAX_WIDTH: usize = 120;

    struct Col {
        title: &'static str,
        center: bool,
        flex: usize,
        base_w: usize,
        w: usize,
    }

    let mut cols = [
        Col { title: " Serial Number", center: false, flex: 0,  base_w: 15, w: 0 },
        Col { title: " Name",          center: false, flex: 15, base_w: 15, w: 0 },
        Col { title: " Device",        center: false, flex: 14, base_w: 14, w: 0 },
        Col { title: " Backup Date",   center: false, flex: 0,  base_w: 18, w: 0 },
        Col { title: "Encrypted",      center: true,  flex: 0,  base_w: 11, w: 0 },
    ];

    let rows: Vec<[String; 5]> = backups
        .iter()
        .map(|(serial, entry)| {
            [
                format!(" {serial}"),
                format!(" {}", entry.info.display_name),
                format!(" {}", entry.info.product_name),
                format!(" {}", utc_to_local(entry.info.last_backup)),
                if entry.manifest.is_encrypted { "Yes".to_string() } else { "No".to_string() },
            ]
        })
        .collect();

    // total target width, clamped; -1 prevents wrapping on the terminal edge
    let target = MIN_WIDTH.max(MAX_WIDTH.min(term_width.saturating_sub(1)));
    let fixed: usize = cols.iter().filter(|c| c.flex == 0).map(|c| c.base_w).sum::<usize>()
        + (cols.len() - 1);
    let mut flex_rem = target as isize - fixed as isize;
    let mut flex_wgt: usize = cols.iter().map(|c| c.flex).sum();

    for i in 0..cols.len() {
        if cols[i].flex > 0 {
            let alloc = flex_rem * cols[i].flex as isize / flex_wgt as isize;
            let max_content = cols[i].base_w.max(
                rows.iter()
                    .map(|r| r[i].chars().count())
                    .max()
                    .unwrap_or(0),
            );
            let w = (alloc.max(0) as usize).min(max_content);
            cols[i].w = w;
            flex_rem -= w as isize;
            flex_wgt -= cols[i].flex;
        } else {
            cols[i].w = cols[i].base_w;
        }
    }

    let fmt = |s: &str, c: &Col| -> String {
        let s: String = if s.chars().count() > c.w {
            let mut t: String = s.chars().take(c.w.saturating_sub(3)).collect();
            t.push_str("...");
            t
        } else {
            s.to_string()
        };
        let pad = c.w.saturating_sub(s.chars().count());
        if c.center {
            format!("{}{}{}", " ".repeat(pad / 2), s, " ".repeat(pad - pad / 2))
        } else {
            format!("{s}{}", " ".repeat(pad))
        }
    };

    let dashes = cols
        .iter()
        .map(|c| "-".repeat(c.w))
        .collect::<Vec<_>>()
        .join(" ");

    let title = "Available iOS Device Backups";
    let pad = dashes.chars().count().saturating_sub(title.chars().count());
    println!("\n{}{}", " ".repeat(pad / 2), title);
    println!("{dashes}");
    println!(
        "{}",
        cols.iter().map(|c| fmt(c.title, c)).collect::<Vec<_>>().join(" ")
    );
    println!("{dashes}");
    for row in &rows {
        println!(
            "{}",
            (0..cols.len())
                .map(|i| fmt(&row[i], &cols[i]))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    println!();
}

fn display_backup_list_long(backups: &BTreeMap<String, BackupEntry>) {
    for (counter, (serial, entry)) in backups.iter().enumerate() {
        println!(
            "{:02}. {}, \"{}\", [{}]",
            counter + 1,
            serial,
            entry.info.display_name,
            entry.info.product_name
        );
        println!("{}", "-".repeat(78));
        println!("    Backup Dir: \"{}\"", entry.location.display());

        for (section, fields) in [
            ("Info", &entry.info.fields),
            ("Manifest", &entry.manifest.fields),
            ("Status", &entry.status.fields),
        ] {
            for (key, value) in fields {
                println!("    {section}.plist/{key}: {value}");
            }
        }
        println!();
    }
}

// ----------------------------------------------------------------
// Pre-extraction checks

fn check_backup_version(version: &str) -> Result<()> {
    let Some(caps) = VERSION_RE.captures(version) else {
        bail!("Error: Unable to determine backup version from Status.plist");
    };
    let major: u32 = caps[1].parse().unwrap_or(0);
    let minor: u32 = caps[2].parse().unwrap_or(0);

    if major < 3 || (major == 3 && minor < 3) {
        bail!(
            concat!("Error: This tool supports iOS backups since version 3.3.\n",
                    "Found version: {major}.{minor}")
        );
    }
    Ok(())
}

// ----------------------------------------------------------------
// iCloud UUID → original filename mapping

fn map_icloud_uuid_to_filename(conn: &Connection, opts: &Options)
-> Result<HashMap<String, String>> {
    let sql = "SELECT relativePath, file FROM files
               WHERE domain='CameraRollDomain'
               AND relativePath LIKE 'Media/PhotoData/CPLAssets/%'";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| anyhow!("Error: 'prepare' method failed on Manifest.db SQLite db:\n\t{e}."))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| anyhow!("Error: 'execute' method failed on Manifest.db SQLite db:\n\t{e}."))?;

    let mut map = HashMap::new();

    while let Some(row) = rows.next()? {
        let relative_path: String = row.get(0)?;
        let blob: Option<Vec<u8>> = row.get(1)?;

        if !EXT_RE.is_match(&relative_path) {
            continue;
        }
        let Some(blob) = blob.filter(|b| !b.is_empty()) else {
            continue;
        };
        let Some(outer) = parse_bplist(Some(&blob), "Manifest.db", opts) else {
            continue;
        };
        let Some(objects) = outer
            .as_dictionary()
            .and_then(|d| d.get("$objects"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        if objects.len() <= 4 {
            continue;
        }
        // $objects[3] holds the raw bytes of a second, embedded binary plist
        let Some(inner_bytes) = objects.get(3).and_then(|v| match v {
            plist::Value::Data(d) => Some(d.clone()),
            plist::Value::String(s) => Some(s.as_bytes().to_vec()),
            _ => None,
        }) else {
            continue;
        };
        let Some(inner) = parse_bplist(Some(&inner_bytes), "Manifest.db", opts) else {
            continue;
        };
        let Some(dict) = inner.as_dictionary() else {
            continue;
        };
        let Some(orig_filename) = dict
            .get("com.apple.assetsd.originalFilename")
            .and_then(|v| v.as_string())
        else {
            continue;
        };
        let stem = orig_filename
            .rsplit_once('.')
            .map(|(base, _)| base)
            .unwrap_or(orig_filename);
        let Some(uuid) = dict
            .get("com.apple.assetsd.cloudAsset.UUID")
            .and_then(|v| v.as_string())
        else {
            continue;
        };
        map.insert(uuid.to_string(), stem.to_string());
    }

    Ok(map)
}

fn replace_icloud_filename(uuid_filename: &str, map: &HashMap<String, String>) -> String {
    let (uuid, extension) = match uuid_filename.rsplit_once('.') {
        Some((base, ext)) => (base, format!(".{ext}")),
        None => (uuid_filename, String::new()),
    };
    match map.get(uuid) {
        Some(orig) => format!("{orig}{extension}"),
        None => uuid_filename.to_string(),
    }
}

// ----------------------------------------------------------------
// Main extraction

fn extract_media_files(opts: &Options, backup: &Backup) -> Result<()> {
    if opts.dry {
        eprintln!("Info: \"--dry\" mode enabled. No files will be copied.");
    }

    if backup.manifest.is_encrypted {
        bail!("Error: Device backup is encrypted. Encrypted backups are not supported.");
    }

    check_backup_version(&backup.status.version)?;

    // databases are copied to a temp dir and opened read-only, so the live
    // backup is never touched (it may be in use by iTunes/Finder)
    let tmp = tempfile::Builder::new()
        .prefix("iOS-Backup-Extractor-")
        .tempdir()
        .context("Error: Cannot create temporary directory")?;
    vinfo(
        opts,
        &format!("Info: Temp directory: {}", tmp.path().display()),
    );

    let aux = create_aux_file_list(opts, &backup.dir, tmp.path());

    let tmp_manifest = tmp.path().join("Manifest.db");
    fs::copy(backup.dir.join("Manifest.db"), &tmp_manifest)
        .map_err(|e| anyhow!("Error: File copy failed: {e}"))?;

    let conn = Connection::open_with_flags(&tmp_manifest, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| {
            anyhow!(
                "Error: Cannot open ‘{}’ as SQLite db: {e}.",
                tmp_manifest.display()
            )
        })?;

    let icloud_map = if opts.ignore_icloud_media {
        HashMap::new()
    } else {
        map_icloud_uuid_to_filename(&conn, opts)?
    };

    let mut extractor = Extractor {
        opts,
        backup_dir: &backup.dir,
        aux: &aux,
        file_index: 1,
    };

    let sql = "SELECT fileID, relativePath, file FROM files
               WHERE domain='CameraRollDomain'
               ORDER BY relativePath ASC";
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| anyhow!("Error: 'prepare' method failed on Manifest.db SQLite db:\n\t{e}."))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| anyhow!("Error: 'execute' method failed on Manifest.db SQLite db:\n\t{e}."))?;

    let mut media_location = String::new();
    let mut media_subdir = String::new();

    while let Some(row) = rows.next()? {
        let file_id: String = row.get(0)?;
        let relative_path: String = row.get(1)?;
        let blob: Option<Vec<u8>> = row.get(2)?;

        let lower = relative_path.to_lowercase();
        if lower.contains("thumb") || lower.contains("metadata") || !EXT_RE.is_match(&relative_path)
        {
            continue;
        }

        let Some(caps) = DCIM_RE
            .captures(&relative_path)
            .or_else(|| ICLD_RE.captures(&relative_path))
        else {
            vwarn(
                opts,
                &format!(
                    concat!("Warning: Cannot determine filename from ",
                            "\"{relative_path}\"\n\tfileID: {file_id}"),
                    relative_path = relative_path,
                    file_id = file_id
                ),
            );
            continue;
        };

        let cap = |name: &str| caps.name(name).map(|m| m.as_str()).unwrap_or("");
        let location = cap("media_location");
        let subdir = cap("media_subdir");
        let re_filename = cap("filename");
        let re_extension = cap("extension");

        let is_icloud_media = location.contains("CPLAssets");
        if is_icloud_media && opts.ignore_icloud_media {
            continue;
        }

        if media_location != location || media_subdir != subdir {
            media_location = location.to_string();
            media_subdir = subdir.to_string();
            eprintln!("  ╭────┤Extracting from: [{media_location}{media_subdir}]");
        }

        let orig_filename = if is_icloud_media {
            replace_icloud_filename(re_filename, &icloud_map)
        } else {
            re_filename.to_string()
        };

        extractor.process(
            &file_id,
            &relative_path,
            blob.as_deref(),
            &orig_filename,
            re_extension,
            false,
        )?;
    }

    // message attachments
    if opts.add_message_media {
        let attachments = list_message_attachments(opts, &backup.dir, tmp.path());
        if !attachments.is_empty() {
            eprintln!("  ╭────┤Extracting from: [Message Attachments]");

            let msg_sql = "SELECT fileID, relativePath, file FROM files
                           WHERE domain='MediaDomain' AND relativePath = ?1";
            let mut msg_stmt = conn.prepare(msg_sql).map_err(|e| {
                anyhow!("Error: 'prepare' method failed on Manifest.db SQLite db:\n\t{e}.")
            })?;

            for rel_path in &attachments {
                let row = msg_stmt.query_row([rel_path], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                });
                if let Ok((file_id, relative_path, blob)) = row {
                    let (orig_filename, extension) = split_msg_filename(&relative_path);
                    extractor.process(
                        &file_id,
                        &relative_path,
                        blob.as_deref(),
                        &orig_filename,
                        &extension,
                        true,
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn split_msg_filename(relative_path: &str) -> (String, String) {
    let last = relative_path.rsplit('/').next().unwrap_or(relative_path);
    match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            (stem.to_string(), ext.to_string())
        }
        _ => ("Unknown".to_string(), "bin".to_string()),
    }
}

// ----------------------------------------------------------------
// Per-file processing

struct Extractor<'a> {
    opts: &'a Options,
    backup_dir: &'a Path,
    aux: &'a AuxData,
    file_index: u32,
}

impl Extractor<'_> {
    fn process(
        &mut self,
        file_id: &str,
        relative_path: &str,
        bplist_blob: Option<&[u8]>,
        orig_filename: &str,
        extension: &str,
        is_message: bool,
    ) -> Result<()> {
        let opts = self.opts;

        // find the file in the blob storage
        if file_id.len() < 3 {
            bail!("Error: Unexpected fileID: {file_id}");
        }
        let subdir = &file_id[..2];
        let blob_file = self.backup_dir.join(subdir).join(file_id);
        if !blob_file.is_file() {
            bail!("Error: \"{}\" doesn't exists", blob_file.display());
        }

        let bplist = parse_bplist(bplist_blob, file_id, opts);

        let file_is_deleted = self.aux.deleted.contains(relative_path);
        let deleted_suffix = if opts.add_trash && file_is_deleted {
            "_DELETED"
        } else {
            ""
        };

        let stem = sanitize_name_for_filesystem(orig_filename);
        let filename = format!("{}{}.{}", stem, deleted_suffix, extension.to_lowercase());

        let last_modified = bplist
            .as_ref()
            .and_then(|v| archived_time(v, "LastModified", file_id, opts));
        let birth = bplist
            .as_ref()
            .and_then(|v| archived_time(v, "Birth", file_id, opts));

        // skip files older than the --since date (unknown dates are kept)
        if let (Some(since), Some(lm)) = (opts.since, last_modified) {
            if lm.date_naive() < since {
                return Ok(());
            }
        }

        // album folder or date-based folder
        let album = if opts.ignore_album_folders {
            None
        } else {
            self.aux.albums.get(relative_path)
        };
        let sub_name = match album {
            Some(album) => sanitize_name_for_filesystem(album),
            None => date_sub_dir(opts.format, last_modified),
        };

        let mut out_sub_dir = opts.out_dir.clone();
        if !sub_name.is_empty() {
            out_sub_dir.push(&sub_name);
        }
        let mut display_sub_dir = sub_name;

        if !opts.dry {
            ensure_dir(&out_sub_dir)?;
            if is_message {
                out_sub_dir.push("Messages");
                ensure_dir(&out_sub_dir)?;
            }
        } else if is_message {
            out_sub_dir.push("Messages");
        }

        if is_message {
            if display_sub_dir.is_empty() {
                display_sub_dir = "Messages".to_string();
            } else {
                display_sub_dir.push_str("/Messages");
            }
        }

        let unique_filename =
            find_unique_filename(&blob_file, &filename, &out_sub_dir, last_modified, opts)?;

        if !opts.add_trash && file_is_deleted {
            println!(
                "{:3}. ({}/{}) {:<13} → <IN_TRASH>, Skipping...",
                self.file_index, subdir, file_id, filename
            );
        } else if unique_filename.is_none() {
            println!(
                "{:3}. ({}/{}) {:<13} → <DUPLICATE>, Skipping...",
                self.file_index, subdir, file_id, filename
            );
        } else {
            let unique_filename = unique_filename.unwrap();
            let display_filename = if display_sub_dir.is_empty() {
                unique_filename.clone()
            } else {
                format!("{display_sub_dir}/{unique_filename}")
            };
            println!(
                "{:3}. ({}/{}) {:<13} → {}",
                self.file_index, subdir, file_id, filename, display_filename
            );

            if !opts.dry {
                let out_file = out_sub_dir.join(&unique_filename);
                copy_file(&blob_file, &out_file)?;
                set_file_attributes(&out_file, last_modified, birth);
            }
        }

        self.file_index += 1;
        Ok(())
    }
}

// ----------------------------------------------------------------
// Message attachments (sms.db)

fn list_message_attachments(opts: &Options, backup_dir: &Path, tmp: &Path) -> Vec<String> {
    let mut attachments = Vec::new();

    let sms_db = tmp.join("sms.db");
    if fs::copy(backup_dir.join(SMS_DB_FILEID), &sms_db).is_err() {
        vwarn(
            opts,
            &format!(
                "Warning: Cannot copy or find sms.db database\n\tfileID: {SMS_DB_FILEID}"
            ),
        );
        return attachments;
    }

    let conn = match Connection::open_with_flags(&sms_db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(e) => {
            vwarn(
                opts,
                &format!(
                    "Warning: Cannot open '{}' as SQLite db: {e}.",
                    sms_db.display()
                ),
            );
            return attachments;
        }
    };

    if !sqlite_table_exists(opts, &conn, "attachment") {
        vwarn(
            opts,
            &format!(
                "Warning: 'attachment' table does not exist in '{}'",
                sms_db.display()
            ),
        );
        return attachments;
    }

    let sql = "SELECT filename FROM attachment
               WHERE filename IS NOT NULL
                 AND filename != ''
                 AND mime_type IS NOT NULL
                 AND mime_type != ''
               ORDER BY ROWID ASC";

    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(e) => {
            vwarn(
                opts,
                &format!(
                    "Warning: 'prepare' method failed on '{}' SQLite db:\n\t{e}.",
                    sms_db.display()
                ),
            );
            return attachments;
        }
    };
    let mut rows = match stmt.query([]) {
        Ok(rows) => rows,
        Err(e) => {
            vwarn(
                opts,
                &format!(
                    "Warning: 'execute' method failed on '{}' SQLite db:\n\t{e}.",
                    sms_db.display()
                ),
            );
            return attachments;
        }
    };

    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let filename: String = row.get(0).unwrap_or_default();
                let filename = filename.strip_prefix("~/").unwrap_or(&filename);
                vinfo(
                    opts,
                    &format!("Info: Message attachment: \"{filename}\""),
                );
                attachments.push(filename.to_string());
            }
            Ok(None) => break,
            Err(e) => {
                vwarn(opts, &format!("Warning: Cannot read attachment row: {e}"));
                break;
            }
        }
    }

    attachments
}

// ----------------------------------------------------------------
// Auxiliary data from Photos.sqlite (trash + albums)

fn create_aux_file_list(opts: &Options, backup_dir: &Path, tmp: &Path) -> AuxData {
    let mut aux = AuxData::default();

    let photos_db = tmp.join("Photos.sqlite");
    if fs::copy(backup_dir.join(PHOTOS_SQLITE_FILEID), &photos_db).is_err() {
        vwarn(
            opts,
            &format!(
                    "Warning: Cannot copy or find Photos.sqlite database\n\tfileID: {}",
                    PHOTOS_SQLITE_FILEID
            ),
        );
        return aux;
    }

    let conn = match Connection::open_with_flags(&photos_db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => conn,
        Err(e) => {
            vwarn(
                opts,
                &format!(
                    "Warning: Cannot open '{}' as SQLite db: {e}.",
                    photos_db.display()
                ),
            );
            return aux;
        }
    };

    create_deleted_file_list(opts, &conn, &mut aux.deleted);
    create_album_file_map(opts, &conn, &mut aux.albums);

    aux
}

fn create_deleted_file_list(opts: &Options, conn: &Connection, deleted: &mut HashSet<String>) {
    if !sqlite_table_exists(opts, conn, "ZASSET") {
        return;
    }

    let sql = "SELECT ZDIRECTORY, ZFILENAME FROM ZASSET WHERE ZTRASHEDSTATE = 1";
    let mut stmt = match conn.prepare(sql) {
        Ok(stmt) => stmt,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'prepare' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };
    let mut rows = match stmt.query([]) {
        Ok(rows) => rows,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'execute' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };

    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let dir: String = row.get(0).unwrap_or_default();
                let file: String = row.get(1).unwrap_or_default();
                let relpath = format!("Media/{dir}/{file}");
                vinfo(
                    opts,
                    &format!("Info: File marked as deleted: {relpath}"),
                );
                deleted.insert(relpath);
            }
            Ok(None) => break,
            Err(e) => {
                vwarn(opts, &format!("Warning: Cannot read ZASSET row: {e}"));
                break;
            }
        }
    }
}

fn create_album_file_map(opts: &Options, conn: &Connection, albums: &mut HashMap<String, String>) {
    if !sqlite_table_exists(opts, conn, "ZGENERICALBUM") {
        vwarn(
            opts,
            "Warning: 'ZGENERICALBUM' table does not exist in Photos.sqlite",
        );
        return;
    }
    if !sqlite_table_exists(opts, conn, "ZASSET") {
        vwarn(opts, "Warning: 'ZASSET' table does not exist in Photos.sqlite");
        return;
    }

    // Apple's Core Data schema assigns the album<->asset junction table a
    // numeric ID that drifts between iOS versions (Z_26ASSETS, Z_29ASSETS...).
    // Find it by shape: a Z_ table with both a Z_<n>ALBUMS and a Z_<n>ASSETS column.
    let mut stmt = match conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'Z_%'")
    {
        Ok(stmt) => stmt,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'prepare' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };
    let names: Vec<String> = match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'execute' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };

    let ident_ok =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');

    let mut junction: Option<(String, String, String)> = None;
    for table in names {
        if !ident_ok(&table) {
            continue;
        }
        let mut pragma = match conn.prepare(&format!(r#"PRAGMA table_info("{table}")"#)) {
            Ok(stmt) => stmt,
            Err(e) => {
                vwarn(
                    opts,
                    &format!("Warning: 'execute' method failed on Photos.sqlite db:\n\t{e}."),
                );
                return;
            }
        };
        let columns: Vec<String> = match pragma.query_map([], |r| r.get::<_, String>(1)) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                vwarn(
                    opts,
                    &format!("Warning: 'execute' method failed on Photos.sqlite db:\n\t{e}."),
                );
                return;
            }
        };

        let album_col = columns.iter().find(|c| {
            c.strip_prefix("Z_")
                .and_then(|rest| rest.strip_suffix("ALBUMS"))
                .is_some_and(|digits| !digits.is_empty()
                                      && digits.bytes().all(|b| b.is_ascii_digit()))
        });
        let asset_col = columns.iter().find(|c| {
            c.strip_prefix("Z_")
                .and_then(|rest| rest.strip_suffix("ASSETS"))
                .is_some_and(|digits| !digits.is_empty()
                                      && digits.bytes().all(|b| b.is_ascii_digit()))
        });

        if let (Some(album_col), Some(asset_col)) = (album_col, asset_col) {
            junction = Some((table, album_col.clone(), asset_col.clone()));
            break;
        }
    }

    let Some((junction_table, album_col, asset_col)) = junction else {
        vwarn(
            opts,
            "Warning: Could not identify junction table in Photos.sqlite",
        );
        return;
    };

    vinfo(
        opts,
        &format!(
            concat!("Info: Album Core Data Schema: Junction Table: \"{junction_table}\", ",
                    "Album Column: \"{album_col}\", Asset Column: \"{asset_col}\""),
            junction_table = junction_table, album_col = album_col, asset_col = asset_col 
        ),
    );

    let query = format!(
        "SELECT
            alb.ZTITLE AS AlbumName,
            'Media/' || ast.ZDIRECTORY || '/' || ast.ZFILENAME AS RelativePath
        FROM ZGENERICALBUM alb
        JOIN \"{junction_table}\" junc ON alb.Z_PK = junc.\"{album_col}\"
        JOIN ZASSET ast ON junc.\"{asset_col}\" = ast.Z_PK
        WHERE alb.ZTITLE IS NOT NULL
          AND alb.ZKIND = 2
          AND ast.ZTRASHEDSTATE = 0"
    );

    let mut stmt = match conn.prepare(&query) {
        Ok(stmt) => stmt,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'prepare' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };
    let mut rows = match stmt.query([]) {
        Ok(rows) => rows,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: 'execute' method failed on Photos.sqlite db:\n\t{e}."),
            );
            return;
        }
    };

    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let album_name: String = row.get(0).unwrap_or_default();
                let relative_path: String = row.get(1).unwrap_or_default();
                vinfo(
                    opts,
                    &format!(
                            concat!("Info: File assigned to album: ",
                                    "\"{relative_path}\" => \"{album_name}\""),
                            relative_path = relative_path,
                            album_name = album_name
                    )
                );
                // a file in multiple albums: last row wins (as in the original)
                albums.insert(relative_path, album_name);
            }
            Ok(None) => break,
            Err(e) => {
                vwarn(opts, &format!("Warning: Cannot read album row: {e}"));
                break;
            }
        }
    }
}

fn sqlite_table_exists(opts: &Options, conn: &Connection, table: &str) -> bool {
    match conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i64>(0),
    ) {
        Ok(count) => count > 0,
        Err(e) => {
            vwarn(
                opts,
                &format!("Warning: Unable to read \"sqlite_master\" table: {e}"),
            );
            false
        }
    }
}

// ----------------------------------------------------------------
// Binary plist helpers

fn parse_bplist(blob: Option<&[u8]>, file_id: &str, opts: &Options) -> Option<plist::Value> {
    let Some(bytes) = blob.filter(|b| !b.is_empty()) else {
        vwarn(
            opts,
            &format!("Warning: Missing file bplist for fileID: {file_id}"),
        );
        return None;
    };

    let value = match plist::Value::from_reader(io::Cursor::new(bytes)) {
        Ok(value) => value,
        Err(_) => {
            vwarn(
                opts,
                &format!("Warning: Cannot parse bplist for fileID: {file_id}"),
            );
            return None;
        }
    };

    if opts.debug {
        let mut xml = Vec::new();
        if value.to_writer_xml(&mut xml).is_ok() {
            eprintln!("Debug: {}", String::from_utf8_lossy(&xml));
        } else {
            // XML cannot represent every node type (e.g. UIDs); dump the structure
            eprintln!("Debug: {value:?}");
        }
    }

    Some(value)
}

fn archived_time(
    value: &plist::Value,
    kind: &str,
    file_id: &str,
    opts: &Options,
) -> Option<DateTime<Local>> {
    let probe = (|| {
        let objects = value.as_dictionary()?.get("$objects")?.as_array()?;
        if objects.len() <= 2 {
            return None;
        }
        let obj = objects.get(1)?.as_dictionary()?;
        // the original accepts only non-negative digit strings (^\d+$)
        let secs = match obj.get(kind)? {
            plist::Value::Integer(i) => i.as_unsigned()?,
            plist::Value::String(s)
                if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) =>
            {
                s.parse::<u64>().ok()?
            }
            _ => return None,
        };
        Local.timestamp_opt(secs as i64, 0).single()
    })();

    if probe.is_none() {
        vwarn(
            opts,
            &format!("Warning: Cannot get {kind} time for fileID: {file_id}"),
        );
    }
    probe
}

// ----------------------------------------------------------------
// Dates

fn parse_since(arg: Option<&str>, verbose: bool) -> Result<Option<NaiveDate>> {
    let Some(arg) = arg else {
        return Ok(None);
    };
    let mut arg = arg.replace('_', "-");

    let lower = arg.to_lowercase();
    if lower == "last-week" || lower == "last-month" {
        let days = if lower == "last-week" { 8 } else { 32 };
        let date = Local::now().date_naive() - Duration::days(days);
        arg = date.format("%Y-%m-%d").to_string();
        if verbose {
            eprintln!("Info: Computed '{lower}' date: \"{arg}\"");
        }
    }

    let invalid = || anyhow!("Error: Invalid --since DATE: \"{arg}\"");
    let Some(caps) = SINCE_RE.captures(&arg) else {
        return Err(invalid());
    };
    let year: i32 = caps[1].parse().map_err(|_| invalid())?;
    let month: u32 = caps[2].parse().map_err(|_| invalid())?;
    let day: u32 = caps[3].parse().map_err(|_| invalid())?;

    if !(1970..3000).contains(&year) {
        return Err(invalid());
    }
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(invalid)?;
    Ok(Some(date))
}

fn date_sub_dir(format: Format, last_modified: Option<DateTime<Local>>) -> String {
    match format {
        Format::Flat => String::new(),
        Format::Ym => match last_modified {
            Some(t) => t.format("%Y-%m").to_string(),
            None => "Unknown_Date".to_string(),
        },
        Format::Ymd => match last_modified {
            Some(t) => t.format("%Y-%m-%d").to_string(),
            None => "Unknown_Date".to_string(),
        },
    }
}

// ----------------------------------------------------------------
// Filename sanitization & unique naming

fn sanitize_name_for_filesystem(name: &str) -> String {
    // 1. normalize Unicode to NFC
    let mut name: String = name.nfc().collect();

    // 2. strip ASCII control characters; 3. replace illegal filesystem characters
    name = name
        .chars()
        .map(|c| {
            let u = c as u32;
            if u < 0x20 || u == 0x7F || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\'
                                                        | '|' | '?' | '*')
            {
                '_'
            } else {
                c
            }
        })
        .collect();

    // 4. trim leading whitespace; 5. trim trailing whitespace and dots
    let mut name = name
        .trim_start_matches(char::is_whitespace)
        .trim_end_matches(|c: char| c.is_whitespace() || c == '.')
        .to_string();

    // 6. prevent unix command-line flag injection
    if let Some(rest) = name.strip_prefix('-') {
        name = format!("_{rest}");
    }

    // 7. fallback for empty or meaningless strings
    if name.is_empty() || name.chars().all(|c| c == '_') {
        name = "Unknown_Name".to_string();
    }

    // 8. safe filesystem byte truncation (max 240 bytes for Ext4/APFS)
    if name.chars().count() > 240 {
        name = name.chars().take(240).collect();
    }
    while name.len() > 240 {
        name.pop(); // pop() drops whole characters, never splits a byte sequence
    }
    let mut name = name
        .trim_end_matches(|c: char| c.is_whitespace() || c == '.')
        .to_string();
    if name.is_empty() || name.chars().all(|c| c == '_') {
        name = "Unknown_Name".to_string();
    }

    // 9. Windows reserved device names
    if RESERVED_NAME_RE.is_match(&name) {
        name = format!("_{name}");
    }

    // 10. unix reserved directory navigation links
    if name == "." || name == ".." {
        name = format!("_{name}");
    }

    name
}

fn find_unique_filename(
    blob_file: &Path,
    filename: &str,
    out_dir: &Path,
    last_modified: Option<DateTime<Local>>,
    opts: &Options,
) -> Result<Option<String>> {
    let mut filename = filename.to_string();
    if opts.prepend_date {
        if let Some(t) = last_modified {
            filename = format!("{}{}", t.format(opts.date_sep.strftime()), filename);
        }
    }

    if !out_dir.join(&filename).exists() {
        return Ok(Some(filename));
    }

    // same name exists: compare content; identical content means duplicate
    let blob_checksum = sha256_file(blob_file)?;
    let Some((base, ext)) = filename.rsplit_once('.') else {
        bail!("Error: Unexpected filename: {filename}");
    };

    let mut proposal = filename.clone();
    let mut index = 0u32;
    while out_dir.join(&proposal).exists() {
        let existing_checksum = sha256_file(&out_dir.join(&proposal))?;
        if blob_checksum == existing_checksum {
            return Ok(None);
        }
        proposal = format!("{base}.{index}.{ext}");
        index += 1;
    }

    Ok(Some(proposal))
}

fn sha256_file(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("Error: Cannot open \"{}\"", path.display()))?;
    let mut reader = io::BufReader::new(file);
    let mut sha = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
    }
    Ok(format!("{:x}", sha.finalize()))
}

// ----------------------------------------------------------------
// Filesystem operations
// (Rust's stdlib uses the wide Unicode Win32 APIs natively, so the
// Perl version's explicit UTF-16 workarounds are not needed here.)

fn ensure_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if dir.exists() {
        bail!(
            "Error: Cannot create directory. A file already exists at: \"{}\"",
            dir.display()
        );
    }
    fs::create_dir(dir)
        .map_err(|e| anyhow!("Error: Failed to create directory \"{}\": {e}", dir.display()))
}

fn copy_file(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to).map_err(|e| {
        anyhow!(
            "Error: Failed to copy \"{}\" to \"{}\": {e}",
            from.display(),
            to.display()
        )
    })?;
    Ok(())
}

fn set_file_attributes(
    file: &Path,
    last: Option<DateTime<Local>>,
    birth: Option<DateTime<Local>>,
) {
    let (Some(last), Some(birth)) = (last, birth) else {
        return;
    };

    let times = fs::FileTimes::new()
        .set_accessed(SystemTime::from(last))
        .set_modified(SystemTime::from(last));
    let _ = fs::set_times(file, times);

    #[cfg(target_os = "macos")]
    {
        let _ = set_birthtime_macos(file, birth);
    }
    #[cfg(windows)]
    {
        let _ = set_creation_time_windows(file, birth);
    }
}

#[cfg(target_os = "macos")]
fn set_birthtime_macos(path: &Path, time: DateTime<Local>) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT as u16;
    attrs.commonattr = libc::ATTR_CMN_CRTIME;
    let ts = libc::timespec {
        tv_sec: time.timestamp(),
        tv_nsec: time.timestamp_subsec_nanos() as _,
    };
    let ret = unsafe {
        libc::setattrlist(
            c_path.as_ptr(),
            &mut attrs as *mut _ as *mut libc::c_void,
            &ts as *const _ as *mut libc::c_void,
            std::mem::size_of::<libc::timespec>(),
            0,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn set_creation_time_windows(path: &Path, time: DateTime<Local>) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::SetFileTime;

    let file = fs::OpenOptions::new().write(true).open(path)?;
    // FILETIME: 100ns intervals since 1601-01-01
    let intervals = (time.timestamp() as u64 + 11_644_473_600) * 10_000_000
        + (time.timestamp_subsec_nanos() / 100) as u64;
    let ft = windows_sys::Win32::Foundation::FILETIME {
        dwLowDateTime: intervals as u32,
        dwHighDateTime: (intervals >> 32) as u32,
    };
    let ok = unsafe {
        SetFileTime(
            file.as_raw_handle() as _,
            &ft,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// ----------------------------------------------------------------
// Console & terminal

fn console_init() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
}

fn is_terminal() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        if io::stdout().is_terminal() {
            return true;
        }
        // MinTTY / Git Bash workaround: such terminals appear as pipes to
        // native Windows programs; ask the MSYS shell to test fd 1 instead.
        #[cfg(windows)]
        {
            let ok = std::process::Command::new("sh")
                .arg("-c")
                .arg("[ -t 1 ]")
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return true;
            }
        }
        false
    })
}

fn terminal_width() -> usize {
    if !is_terminal() {
        return 100;
    }
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
}

fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "ios_backup_extractor".to_string())
}

// ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_plain() {
        assert_eq!(sanitize_name_for_filesystem("IMG_1234"), "IMG_1234");
    }

    #[test]
    fn sanitize_illegal_chars() {
        assert_eq!(
            sanitize_name_for_filesystem("a<b>:c\"d/e\\f|g?h*i"),
            "a_b__c_d_e_f_g_h_i"
        );
        assert_eq!(sanitize_name_for_filesystem("a\u{0}b\u{7f}c"), "a_b_c");
    }

    #[test]
    fn sanitize_trimming_and_fallback() {
        assert_eq!(sanitize_name_for_filesystem("  foo.  "), "foo");
        assert_eq!(sanitize_name_for_filesystem("-flag"), "_flag");
        assert_eq!(sanitize_name_for_filesystem("???"), "Unknown_Name");
        assert_eq!(sanitize_name_for_filesystem("..."), "Unknown_Name");
        assert_eq!(sanitize_name_for_filesystem(""), "Unknown_Name");
    }

    #[test]
    fn sanitize_reserved_names() {
        assert_eq!(sanitize_name_for_filesystem("CON"), "_CON");
        assert_eq!(sanitize_name_for_filesystem("com1.txt"), "_com1.txt");
        assert_eq!(sanitize_name_for_filesystem("LPT9"), "_LPT9");
        assert_eq!(sanitize_name_for_filesystem("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_name_for_filesystem("COM0"), "COM0");
    }

    #[test]
    fn sanitize_unicode_truncation() {
        let name = "é".repeat(300);
        let result = sanitize_name_for_filesystem(&name);
        assert!(result.len() <= 240);
        assert_eq!(result.chars().count(), 120); // 2 bytes per char

        let emoji = "😀".repeat(100);
        let result = sanitize_name_for_filesystem(&emoji);
        assert!(result.len() <= 240);
        assert_eq!(result.chars().count(), 60); // 4 bytes per char
    }

    #[test]
    fn since_parsing() {
        assert!(parse_since(Some("2024-02-29"), false).unwrap().is_some());
        assert!(parse_since(Some("2023-02-29"), false).is_err());
        assert!(parse_since(Some("2024_1_2"), false).unwrap().is_some());
        assert!(parse_since(Some("1969-01-01"), false).is_err());
        assert!(parse_since(Some("2024-13-01"), false).is_err());
        assert!(parse_since(None, false).unwrap().is_none());
    }

    #[test]
    fn classify_dcim() {
        let caps = DCIM_RE
            .captures("Media/DCIM/101APPLE/IMG_1111.JPG")
            .unwrap();
        assert_eq!(&caps["media_location"], "Media/DCIM/");
        assert_eq!(&caps["media_subdir"], "101APPLE/");
        assert_eq!(&caps["filename"], "IMG_1111");
        assert_eq!(&caps["extension"], "JPG");
    }

    #[test]
    fn classify_dcim_mutations() {
        let caps = DCIM_RE
            .captures(
                "Media/PhotoData/Mutations/DCIM/101APPLE/IMG_1111/Adjustments/FullSizeRender.jpg",
            )
            .unwrap();
        assert_eq!(&caps["filename"], "IMG_1111");
        assert_eq!(&caps["extension"], "jpg");
    }

    #[test]
    fn classify_icloud() {
        let caps = ICLD_RE
            .captures("Media/PhotoData/CPLAssets/group101/1A1A1A1A-1A1A.HEIC")
            .unwrap();
        assert_eq!(&caps["media_subdir"], "group101/");
        assert_eq!(&caps["filename"], "1A1A1A1A-1A1A");
        assert_eq!(&caps["extension"], "HEIC");
        assert!(DCIM_RE
            .captures("Media/PhotoData/CPLAssets/group101/1A1A1A1A-1A1A.HEIC")
            .is_none());
    }

    #[test]
    fn extension_filter() {
        assert!(EXT_RE.is_match("IMG_0001.HEIC"));
        assert!(EXT_RE.is_match("video.MP4"));
        assert!(!EXT_RE.is_match("doc.pdf"));
        assert!(!EXT_RE.is_match("IMG_0001.heic.tmp"));
    }

    #[test]
    fn unique_filename_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();
        let blob = tmp.path().join("blob");
        fs::write(&blob, b"content").unwrap();
        let opts = Options::default();

        // free name
        let name = find_unique_filename(&blob, "x.jpg", &out, None, &opts)
            .unwrap()
            .unwrap();
        assert_eq!(name, "x.jpg");

        // identical content -> duplicate
        fs::write(out.join("x.jpg"), b"content").unwrap();
        assert!(find_unique_filename(&blob, "x.jpg", &out, None, &opts)
            .unwrap()
            .is_none());

        // different content -> numbered name
        fs::write(out.join("x.jpg"), b"other").unwrap();
        let name = find_unique_filename(&blob, "x.jpg", &out, None, &opts)
            .unwrap()
            .unwrap();
        assert_eq!(name, "x.0.jpg");
    }

    #[test]
    fn msg_filename_split() {
        assert_eq!(
            split_msg_filename("Library/SMS/Attachments/ab/cd/IMG_0001.JPG"),
            ("IMG_0001".to_string(), "JPG".to_string())
        );
        assert_eq!(
            split_msg_filename("Library/SMS/Attachments/.hidden"),
            ("Unknown".to_string(), "bin".to_string())
        );
    }

    #[test]
    fn icloud_filename_replacement() {
        let mut map = HashMap::new();
        map.insert("UUID-1".to_string(), "IMG_5555".to_string());
        assert_eq!(replace_icloud_filename("UUID-1.HEIC", &map), "IMG_5555.HEIC");
        assert_eq!(replace_icloud_filename("UUID-2.HEIC", &map), "UUID-2.HEIC");
    }

    #[test]
    fn backup_version_check() {
        assert!(check_backup_version("3.3").is_ok());
        assert!(check_backup_version("4.0").is_ok());
        assert!(check_backup_version("3.2").is_err());
        assert!(check_backup_version("2.9").is_err());
        assert!(check_backup_version("").is_err());
    }
}