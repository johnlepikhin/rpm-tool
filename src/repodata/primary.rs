use std::os::linux::fs::MetadataExt;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use slog_scope::info;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
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

impl<T> Default for Tagged<Vec<T>> {
    fn default() -> Self {
        Self {
            value: Default::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename = "version")]
pub struct PackageVersion {
    #[serde(rename = "@epoch")]
    pub epoch: i32,
    #[serde(rename = "@ver")]
    pub ver: String,
    #[serde(rename = "@rel")]
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PackageChecksum {
    #[serde(rename = "@type")]
    pub type_: String,
    #[serde(rename = "@pkgid")]
    pub pkgid: String,
    #[serde(rename = "$value")]
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PackageTime {
    #[serde(rename = "@file")]
    pub file: i64,
    #[serde(rename = "@build")]
    pub build: u32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PackageSize {
    #[serde(rename = "@package")]
    pub package: u64,
    #[serde(rename = "@installed")]
    pub installed: u64,
    #[serde(rename = "@archive")]
    pub archive: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename(serialize = "rpm:entry", deserialize = "entry"))]
pub struct RpmEntry {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@flags", skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    #[serde(rename = "@epoch", skip_serializing_if = "Option::is_none")]
    pub epoch: Option<String>,
    #[serde(rename = "@ver", skip_serializing_if = "Option::is_none")]
    pub ver: Option<String>,
    #[serde(rename = "@rel", skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
    #[serde(rename = "@pre", skip_serializing_if = "Option::is_none")]
    pub pre: Option<u8>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RpmEntryList {
    #[serde(default, rename(serialize = "rpm:entry", deserialize = "entry"))]
    pub list: Vec<RpmEntry>,
}

impl From<Vec<RpmEntry>> for RpmEntryList {
    fn from(list: Vec<RpmEntry>) -> Self {
        Self { list }
    }
}

impl RpmEntry {
    fn encode_flags(v: i32) -> Result<Option<String>> {
        let r = match v & 0x0f {
            0 => return Ok(None),
            2 => "LT",
            4 => "GT",
            8 => "EQ",
            10 => "LE",
            12 => "GE",
            _ => bail!("Invalid flag value {:?}", v),
        };
        Ok(Some(r.to_owned()))
    }

    fn nonempty_or_none(v: Option<&str>) -> Option<String> {
        v.and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(v.to_owned())
            }
        })
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

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct FileEntry {
    #[serde(rename = "$value")]
    pub path: std::path::PathBuf,
}

impl FileEntry {
    pub fn of_rpm_file_entry(entry: rpm::FileEntry) -> Result<Self> {
        Ok(Self { path: entry.path })
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PackageFormat {
    #[serde(default, rename(serialize = "rpm:license", deserialize = "license"))]
    pub rpm_license: Option<String>,
    #[serde(default, rename(serialize = "rpm:vendor", deserialize = "vendor"))]
    pub rpm_vendor: Option<String>,
    #[serde(default, rename(serialize = "rpm:group", deserialize = "group"))]
    pub rpm_group: Option<String>,
    #[serde(
        default,
        rename(serialize = "rpm:buildhost", deserialize = "buildhost")
    )]
    pub rpm_buildhost: Option<String>,
    #[serde(
        default,
        rename(serialize = "rpm:sourcerpm", deserialize = "sourcerpm")
    )]
    pub rpm_sourcerpm: Option<String>,
    // TODO
    // #[serde(skip_serializing_if = "Option::is_none", rename = "rpm:header-range")]
    // pub rpm_header_range: Option<Tagged<String>>,
    #[serde(default, rename(serialize = "rpm:provides", deserialize = "provides"))]
    pub rpm_provides: RpmEntryList,
    #[serde(
        default,
        rename(serialize = "rpm:conflicts", deserialize = "conflicts")
    )]
    pub rpm_conflicts: RpmEntryList,
    #[serde(
        default,
        rename(serialize = "rpm:obsoletes", deserialize = "obsoletes")
    )]
    pub rpm_obsoletes: RpmEntryList,
    #[serde(default, rename(serialize = "rpm:requires", deserialize = "requires"))]
    pub rpm_requires: RpmEntryList,
    #[serde(default, rename = "file")]
    pub files: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct PackageLocation {
    #[serde(rename = "@href")]
    pub href: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename = "package")]
pub struct Package {
    #[serde(rename = "@type")]
    pub type_: String,
    pub name: Tagged<String>,
    pub location: PackageLocation,
    pub arch: Option<Tagged<String>>,
    pub description: Tagged<Option<String>>,
    pub version: PackageVersion,
    pub checksum: PackageChecksum,
    pub summary: Tagged<Option<String>>,
    #[serde(default)]
    pub packager: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    pub time: PackageTime,
    pub size: PackageSize,
    pub format: PackageFormat,
}

impl Package {
    fn useful_file(entry: &rpm::FileEntry, regex: &regex::Regex) -> bool {
        regex.is_match(entry.path.to_string_lossy().as_ref())
    }

    pub fn of_rpm_package(
        pkg: &rpm::RPMPackage,
        path: &std::path::Path,
        relative_path: &std::path::Path,
        file_sha: &str,
        useful_files: &regex::Regex,
    ) -> Result<Self> {
        let header = &pkg.metadata.header;

        let metadata = path.metadata()?;

        let time = PackageTime {
            file: metadata.st_mtime(),
            build: header
                .get_build_time()
                .map_err(|err| anyhow!("{}", err.to_string()))?,
        };

        let size = PackageSize {
            archive: header.get_archive_size().ok(),
            installed: header
                .get_installed_size()
                .map_err(|err| anyhow!("{}", err.to_string()))?,
            package: metadata.st_size(),
        };

        let rpm_provides = header
            .get_provides_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Provision entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<Vec<_>>>()?
            .into();

        let rpm_conflicts = header
            .get_conflicts_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Conflict entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<Vec<_>>>()?
            .into();

        let rpm_obsoletes = header
            .get_obsoletes_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                RpmEntry::of_rpmentry(&v)
                    .map_err(|err| anyhow!("Obsolutes entry {:?}: {}", &v.name, err))
            })
            .collect::<Result<Vec<_>>>()?
            .into();

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
            .collect::<Result<Vec<_>>>()?
            .into();

        let files: Vec<_> = header
            .get_file_entries()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| Self::useful_file(f, useful_files))
            .map(FileEntry::of_rpm_file_entry)
            .collect::<Result<_>>()?;

        let format = PackageFormat {
            rpm_license: header.get_license().ok().map(|v| v.to_owned()),
            rpm_vendor: header.get_vendor().ok().map(|v| v.to_owned()),
            rpm_group: header.get_group().unwrap_or_default().join("").into(),
            rpm_buildhost: header.get_buildhost().ok().map(|v| v.to_owned()),
            rpm_sourcerpm: header.get_source_rpm().ok().map(|v| v.to_owned()),
            rpm_provides,
            rpm_conflicts,
            rpm_obsoletes,
            rpm_requires,
            files,
        };

        let r = Self {
            type_: "rpm".to_owned(),
            name: header.get_name().ok().into(),
            location: PackageLocation {
                href: relative_path.to_string_lossy().to_string(),
            },
            arch: header.get_arch().map(|v| v.to_owned().into()).ok(),
            description: Some(
                header
                    .get_description()
                    .map_err(|err| anyhow!("{}", err.to_string()))?
                    .join(""),
            )
            .into(),
            version: PackageVersion::of_header(header)
                .map_err(|err| anyhow!("{}", err.to_string()))?,
            checksum: PackageChecksum {
                type_: "sha".to_owned(),
                pkgid: "YES".to_owned(),
                value: file_sha.to_owned(),
            },
            summary: Some(
                header
                    .get_summary()
                    .map_err(|err| anyhow!("{}", err.to_string()))?
                    .join(""),
            )
            .into(),
            packager: header.get_packager().unwrap_or_default().join("").into(),
            url: header.get_url().ok().map(|v| v.to_owned()),
            time,
            size,
            format,
        };
        Ok(r)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename = "metadata")]
pub struct Primary {
    #[serde(rename = "@xmlns")]
    pub xmlns: String,
    #[serde(rename(deserialize = "@rpm", serialize = "@xmlns:rpm"))]
    pub xmlns_url: String,
    #[serde(rename = "@packages")]
    pub packages: usize,
    #[serde(default)]
    pub package: Vec<Package>,
}

impl Primary {
    pub fn new() -> Self {
        Self {
            xmlns: "http://linux.duke.edu/metadata/common".to_owned(),
            xmlns_url: "http://linux.duke.edu/metadata/rpm".to_owned(),
            packages: 0,
            package: Vec::new(),
        }
    }

    pub fn add_package(&mut self, package: Package) {
        self.packages += 1;
        self.package.push(package)
    }

    pub fn drain_filter<F>(&mut self, pred: F) -> Vec<Package>
    where
        F: Fn(&Package) -> bool,
    {
        let mut drained = Vec::new();
        let mut keep = Vec::new();

        for package in self.package.drain(..) {
            if pred(&package) {
                keep.push(package)
            } else {
                drained.push(package)
            }
        }
        self.packages = keep.len();
        self.package = keep;

        drained
    }

    pub fn read(path: &std::path::Path) -> Result<Self> {
        info!("Reading primary metadata from {:?}", path);
        let file = std::fs::File::open(path)?;
        let reader = flate2::read::GzDecoder::new(file);
        let buf_reader = std::io::BufReader::new(reader);
        let r = quick_xml::de::from_reader(buf_reader)?;
        Ok(r)
    }
}

#[test]
fn test_de_rpm_entry() {
    let r: RpmEntry = quick_xml::de::from_str(
        r#"
<rpm:entry name="attr-debuginfo" flags="EQ" epoch="0" ver="2.4.46" rel="13.vk1.el7"/>
"#,
    )
    .unwrap();

    println!("{}", quick_xml::se::to_string(&r).unwrap());

    assert_eq!(
        r,
        RpmEntry {
            name: "attr-debuginfo".to_owned(),
            flags: Some("EQ".to_owned()),
            epoch: Some("0".to_owned()),
            ver: Some("2.4.46".to_owned()),
            rel: Some("13.vk1.el7".to_owned()),
            pre: None
        }
    )
}

#[test]
fn test_de_empty_metadata() {
    let r: Primary = quick_xml::de::from_str(
        r#"<metadata xmlns="http://linux.duke.edu/metadata/common" xmlns:rpm="http://linux.duke.edu/metadata/rpm" packages="302"></metadata>"#,
    ).unwrap();

    assert_eq!(
        r,
        Primary {
            xmlns: "http://linux.duke.edu/metadata/common".to_owned(),
            xmlns_url: "http://linux.duke.edu/metadata/rpm".to_owned(),
            packages: 302,
            package: Vec::new()
        }
    )
}

#[test]
fn test_de_metadata_one_package() {
    let r: Primary = quick_xml::de::from_str(
        r#"
<metadata xmlns="http://linux.duke.edu/metadata/common" xmlns:rpm="http://linux.duke.edu/metadata/rpm" packages="302">
<package type="rpm">
  <name>v8_monolith</name>
  <arch>x86_64</arch>
  <version epoch="0" ver="10.3.174.14" rel="1"/>
  <checksum type="sha" pkgid="YES">bff3977e704f06e9f8ff51ee365c4ab419e91225</checksum>
  <summary>JavaScript Engine</summary>
  <description>V8 is Google's open source high-performance JavaScript engine, written in C++ and used in Google Chrome, the open source browser from
Google. It implements ECMAScript as specified in ECMA-262, 3rd edition, and runs on Windows XP or later, Mac OS X 10.5+, and Linux systems
that use IA-32, ARM or MIPS processors. V8 can run standalone, or can be embedded into any C++ application.</description>
  <packager></packager>
  <url></url>
  <time file="1657717375" build="1655985827"/>
  <size package="8940944" installed="62249667" archive="62259544"/>
  <location href="v8_monolith-10.3.174.14-1.x86_64.rpm"/>
  <format>
    <rpm:license>BSD</rpm:license>
    <rpm:vendor></rpm:vendor>
    <rpm:group>System Environment/Libraries</rpm:group>
    <rpm:buildhost>some.host</rpm:buildhost>
    <rpm:sourcerpm>v8_monolith-10.3.174.14-1.src.rpm</rpm:sourcerpm>
    <rpm:header-range start="4504" end="15636"/>
    <rpm:provides>
      <rpm:entry name="v8_monolith" flags="EQ" epoch="0" ver="10.3.174.14" rel="1"/>
      <rpm:entry name="v8_monolith(x86-64)" flags="EQ" epoch="0" ver="10.3.174.14" rel="1"/>
    </rpm:provides>
  </format>
</package>
</metadata>
"#,
    ).unwrap();

    println!("{}", quick_xml::se::to_string(&r).unwrap());

    let provides_list = vec![
        RpmEntry {
            name: "v8_monolith".to_owned(),
            flags: Some("EQ".to_owned()),
            epoch: Some("0".to_owned()),
            ver: Some("10.3.174.14".to_owned()),
            rel: Some("1".to_owned()),
            pre: None,
        },
        RpmEntry {
            name: "v8_monolith(x86-64)".to_owned(),
            flags: Some("EQ".to_owned()),
            epoch: Some("0".to_owned()),
            ver: Some("10.3.174.14".to_owned()),
            rel: Some("1".to_owned()),
            pre: None,
        },
    ];

    let expected_package =
        Package {
            type_: "rpm".to_owned(),
            name: Tagged { value: "v8_monolith".to_owned() },
            location: PackageLocation { href: "v8_monolith-10.3.174.14-1.x86_64.rpm".to_owned() },
            arch: Some(Tagged { value: "x86_64".to_owned() }),
            description: Tagged { value: Some(r#"V8 is Google's open source high-performance JavaScript engine, written in C++ and used in Google Chrome, the open source browser from
Google. It implements ECMAScript as specified in ECMA-262, 3rd edition, and runs on Windows XP or later, Mac OS X 10.5+, and Linux systems
that use IA-32, ARM or MIPS processors. V8 can run standalone, or can be embedded into any C++ application."#.to_owned()) },
            version: PackageVersion { epoch: 0, ver: "10.3.174.14".to_owned(), rel: "1".to_owned() },
            checksum: PackageChecksum { type_: "sha".to_owned(), pkgid: "YES".to_owned(), value: "bff3977e704f06e9f8ff51ee365c4ab419e91225".to_owned() },
            summary: Tagged { value: Some("JavaScript Engine".to_owned()) },
            packager: Some("".to_owned()),
            url: Some("".to_owned()),
            time: PackageTime { file: 1657717375, build: 1655985827 },
            size: PackageSize { package: 8940944, installed: 62249667, archive: Some(62259544) },
            format: PackageFormat {
                rpm_license: Some("BSD".to_owned()),
                rpm_vendor: Some("".to_owned()),
                rpm_group: Some("System Environment/Libraries".to_owned()),
                rpm_buildhost: Some("some.host".to_owned()),
                rpm_sourcerpm: Some("v8_monolith-10.3.174.14-1.src.rpm".to_owned()),
                rpm_provides: RpmEntryList { list: provides_list },
                rpm_conflicts: Default::default(),
                rpm_obsoletes: Default::default(),
                rpm_requires: Default::default(),
                files: Default::default()
            }
        };

    assert_eq!(
        r,
        Primary {
            xmlns: "http://linux.duke.edu/metadata/common".to_owned(),
            xmlns_url: "http://linux.duke.edu/metadata/rpm".to_owned(),
            packages: 302,
            package: vec![expected_package]
        }
    )
}
