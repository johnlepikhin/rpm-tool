use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename = "package")]
pub struct Package {
    #[serde(rename = "@pkgid")]
    pub pkgid: String,
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, rename = "@arch")]
    pub arch: Option<String>,
    pub version: crate::repodata::primary::PackageVersion,
    #[serde(default, rename = "file")]
    pub files: Vec<crate::repodata::primary::FileEntry>,
}

impl Package {
    pub fn of_rpm_package(pkg: &rpm::RPMPackage, file_sha: &str) -> Result<Self> {
        let header = &pkg.metadata.header;

        let files: Vec<_> = header
            .get_file_entries()
            .unwrap_or_default()
            .into_iter()
            .map(super::primary::FileEntry::of_rpm_file_entry)
            .collect::<Result<_>>()?;

        let r = Self {
            name: header
                .get_name()
                .map_err(|err| anyhow!("Cannot extract package name: {}", err))?
                .to_owned(),
            arch: header.get_arch().map(|v| v.to_owned()).ok(),
            version: super::primary::PackageVersion::of_header(header)
                .map_err(|err| anyhow!("{}", err.to_string()))?,
            files,
            pkgid: file_sha.to_owned(),
        };
        Ok(r)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename = "filelists")]
pub struct Filelists {
    #[serde(rename = "@xmlns")]
    pub xmlns: String,
    #[serde(rename = "@packages")]
    pub packages: usize,
    #[serde(default)]
    pub package: Vec<Package>,
}

impl Filelists {
    pub fn new() -> Self {
        Self {
            xmlns: "http://linux.duke.edu/metadata/filelists".to_owned(),
            packages: 0,
            package: Vec::new(),
        }
    }

    pub fn add_package(&mut self, package: Package) {
        self.packages += 1;
        self.package.push(package)
    }
}
