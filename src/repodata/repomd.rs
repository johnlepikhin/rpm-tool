use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Checksum {
    #[serde(rename = "@type")]
    pub type_: String,
    #[serde(rename = "$value")]
    pub value: String,
}

impl Checksum {
    pub fn new(value: String) -> Self {
        Self {
            type_: "sha".to_owned(),
            value,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Location {
    #[serde(rename = "@href")]
    pub href: String,
}

impl Location {
    pub fn new(href: String) -> Self {
        Self { href }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum DataType {
    #[serde(rename = "primary")]
    Primary,
    #[serde(rename = "filelists")]
    Filelists,
    #[serde(rename = "other")]
    Other,
    #[serde(rename = "primary_db")]
    PrimaryDb,
    #[serde(rename = "filelists_db")]
    FilelistsDb,
    #[serde(rename = "other_db")]
    OtherDb,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename = "package")]
pub struct Data {
    #[serde(rename = "@type")]
    pub type_: DataType,
    #[serde(rename = "checksum")]
    pub checksum: Checksum,
    #[serde(rename = "open-checksum")]
    pub open_checksum: Checksum,
    #[serde(rename = "location")]
    pub location: Location,
    #[serde(rename = "timestamp")]
    pub timestamp: i64,
    #[serde(rename = "size")]
    pub size: u64,
    #[serde(rename = "open-size")]
    pub open_size: usize,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename = "repomd")]
pub struct Repomd {
    #[serde(rename = "@xmlns")]
    pub xmlns: String,
    #[serde(rename(deserialize = "@rpm", serialize = "@xmlns:rpm"))]
    pub xmlns_url: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub data: Vec<Data>,
}

impl Repomd {
    pub fn new() -> Self {
        Self {
            xmlns: "http://linux.duke.edu/metadata/common".to_owned(),
            xmlns_url: "http://linux.duke.edu/metadata/rpm".to_owned(),
            revision: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            data: Vec::new(),
        }
    }

    pub fn add_data(&mut self, data: Data) {
        self.data.push(data)
    }
}
