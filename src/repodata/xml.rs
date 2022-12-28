use std::os::linux::fs::MetadataExt;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Tagged<T> {
    #[serde(rename = "$value")]
    pub value: T,
}

impl<T> From<T> for Tagged<T> {
    fn from(value: T) -> Self {
        Self { value }
    }
}

impl From<Option<&str>> for Tagged<String> {
    fn from(value: Option<&str>) -> Self {
        Self {
            value: value.map(|v| v.to_owned()).unwrap_or_default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename = "version")]
pub struct PackageVersion {
    pub epoch: i32,
    pub ver: String,
    pub rel: String,
}

impl PackageVersion {
    pub fn of_header(
        header: &rpm::Header<rpm::IndexTag>,
    ) -> std::result::Result<Self, rpm::RPMError> {
        let r = Self {
            epoch: header.get_epoch().unwrap_or_default(),
            ver: header.get_version()?.to_owned(),
            rel: header.get_release()?.to_owned(),
        };
        Ok(r)
    }
}

#[derive(Serialize, Deserialize)]
pub struct PackageChecksum {
    #[serde(rename = "type")]
    pub type_: String,
    pub pkgid: String,
    #[serde(rename = "$value")]
    pub value: String,
}

#[derive(Serialize, Deserialize)]
pub struct PackageTime {
    pub file: i64,
    pub build: u32,
}

#[derive(Serialize, Deserialize)]
pub struct PackageSize {
    pub package: u64,
    pub installed: u64,
    pub archive: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename = "rpm:entry")]
pub struct RpmEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre: Option<u8>,
}

impl RpmEntry {
    fn encode_flags(v: i32) -> Result<Option<String>> {
        let r = match v & 0x0f {
            0 => return Ok(None),
            2 => "LT",
            4 => "GT",
            8 => "EC",
            10 => "LE",
            12 => "GE",
            _ => bail!("Invalid flag value {:?}", v),
        };
        Ok(Some(r.to_owned()))
    }

    fn nonempty_or_none(v: Option<&str>) -> Option<String> {
        v.map(|v| {
            if v.is_empty() {
                None
            } else {
                Some(v.to_owned())
            }
        })
        .flatten()
    }

    pub fn of_rpmentry(v: &rpm::RpmEntry) -> Result<Self> {
        lazy_static::lazy_static! {
            static ref VERSION_RE: regex::Regex = regex::Regex::new("^(:?(\\d+):)?(.+?)(:?-(.+))?$").unwrap();
        }

        let (epoch, ver, rel) = if v.version.is_empty() {
            (None, None, None)
        } else {
            let version_caps = match VERSION_RE.captures(&v.version) {
                Some(v) => v,
                None => bail!("Cannot parse version {:?}", v.version),
            };
            (
                Self::nonempty_or_none(version_caps.get(2).map(|v| v.as_str())),
                Self::nonempty_or_none(version_caps.get(3).map(|v| v.as_str())),
                Self::nonempty_or_none(version_caps.get(5).map(|v| v.as_str())),
            )
        };

        let pre = if v.flags & 1024 > 0 { Some(1) } else { None };

        Ok(Self {
            name: v.name.clone(),
            flags: Self::encode_flags(v.flags)?,
            epoch,
            ver,
            rel,
            pre,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct FileEntry {
    #[serde(rename = "$value")]
    pub path: std::path::PathBuf,
}

impl FileEntry {
    pub fn of_rpm_file_entry(entry: rpm::FileEntry) -> Result<Self> {
        Ok(Self {
            path: entry.path.into(),
        })
    }
}

#[derive(Serialize, Deserialize)]
pub struct PackageFormat {
    #[serde(rename = "rpm:license")]
    pub rpm_license: Tagged<String>,
    #[serde(rename = "rpm:vendor")]
    pub rpm_vendor: Tagged<String>,
    #[serde(rename = "rpm:group")]
    pub rpm_group: Tagged<String>,
    #[serde(rename = "rpm:buildhost")]
    pub rpm_buildhost: Tagged<String>,
    #[serde(rename = "rpm:sourcerpm")]
    pub rpm_sourcerpm: Tagged<String>,
    // TODO
    // #[serde(skip_serializing_if = "Option::is_none", rename = "rpm:header-range")]
    // pub rpm_header_range: Option<Tagged<String>>,
    #[serde(rename = "rpm:provides")]
    pub rpm_provides: Vec<RpmEntry>,
    #[serde(rename = "rpm:conflicts")]
    pub rpm_conflicts: Vec<RpmEntry>,
    #[serde(rename = "rpm:obsoletes")]
    pub rpm_obsoletes: Vec<RpmEntry>,
    #[serde(rename = "rpm:requires")]
    pub rpm_requires: Vec<RpmEntry>,
    #[serde(rename = "file")]
    pub files: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename = "package")]
pub struct Package {
    #[serde(rename = "type")]
    pub type_: String,
    pub name: Tagged<String>,
    pub arch: Option<Tagged<String>>,
    pub description: Tagged<String>,
    pub version: PackageVersion,
    pub checksum: Option<PackageChecksum>,
    pub summary: Tagged<String>,
    pub packager: Option<Tagged<String>>,
    pub url: Option<Tagged<String>>,
    pub time: PackageTime,
    pub size: PackageSize,
    pub format: PackageFormat,
}

impl Package {
    fn useful_file(entry: &rpm::FileEntry) -> bool {
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new("(?:^/etc|/bin/|^/usr/lib/sendmail$)").unwrap();
        }

        RE.is_match(entry.path.to_string_lossy().as_ref())
    }

    pub fn of_rpm_package(pkg: &rpm::RPMPackage, rpm_file: &std::fs::File) -> Result<Self> {
        let header = &pkg.metadata.header;

        let time = PackageTime {
            file: rpm_file.metadata()?.st_mtime(),
            build: header
                .get_build_time()
                .map_err(|err| anyhow!("{}", err.to_string()))?,
        };

        let size = PackageSize {
            archive: header.get_archive_size().ok(),
            installed: header
                .get_installed_size()
                .map_err(|err| anyhow!("{}", err.to_string()))?,
            package: rpm_file.metadata()?.st_size(),
        };

        let rpm_provides = header
            .get_provides_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Provision entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<_>>()?;

        let rpm_conflicts = header
            .get_conflicts_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Conflict entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<_>>()?;

        let rpm_obsoletes = header
            .get_obsoletes_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Obsolutes entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<_>>()?;

        let rpm_requires = header
            .get_requires_entries()
            .unwrap_or_default()
            .into_iter()
            // Skip rpm specific requirements
            .filter(|v| v.flags & 16777216 == 0)
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Requires entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<_>>()?;

        let files = header
            .get_file_entries()
            .unwrap_or_default()
            .into_iter()
            .filter(Self::useful_file)
            .map(|v| FileEntry::of_rpm_file_entry(v))
            .collect::<Result<_>>()?;

        let format = PackageFormat {
            rpm_license: header.get_license().ok().into(),
            rpm_vendor: header.get_vendor().ok().into(),
            rpm_group: header.get_group().unwrap_or_default().join("").into(),
            rpm_buildhost: header.get_buildhost().ok().into(),
            rpm_sourcerpm: header.get_source_rpm().ok().into(),
            rpm_provides,
            rpm_conflicts,
            rpm_obsoletes,
            rpm_requires,
            files,
        };

        let r = Self {
            type_: "rpm".to_owned(),
            name: "attr".to_owned().into(),
            arch: header.get_arch().map(|v| v.to_owned().into()).ok(),
            description: header
                .get_description()
                .map_err(|err| anyhow!("{}", err.to_string()))?
                .join("")
                .into(),
            version: PackageVersion::of_header(&header)
                .map_err(|err| anyhow!("{}", err.to_string()))?,
            // TODO
            checksum: None,
            summary: header
                .get_summary()
                .map_err(|err| anyhow!("{}", err.to_string()))?
                .join("")
                .into(),
            packager: header.get_packager().ok().map(|v| v.join("").into()),
            url: header.get_url().ok().map(|v| v.to_owned().into()),
            time,
            size,
            format,
        };
        Ok(r)
    }
}
