pub mod xml;

use anyhow::{anyhow, Context, Result};
use gzp::{
    deflate::Gzip,
    par::compress::{ParCompress, ParCompressBuilder},
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use slog_scope::{error, info, warn};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

pub struct State {
    repo_path: std::path::PathBuf,
    tempdir: tempfile::TempDir,
    primary_xml: Arc<Mutex<ParCompress<Gzip>>>,
}

impl State {
    pub fn new(repo_path: &std::path::Path) -> Result<Self> {
        let tempdir = tempfile::Builder::new()
            .prefix(".repodata_")
            .tempdir_in(repo_path)?;

        info!("Will generate new repository index in {:?}", tempdir.path());

        let primary_xml = std::fs::File::create(tempdir.path().join("primary.xml.gz"))?;
        let primary_xml: ParCompress<Gzip> = ParCompressBuilder::new().from_writer(primary_xml);

        let r = Self {
            tempdir,
            repo_path: repo_path.to_path_buf(),
            primary_xml: Arc::new(Mutex::new(primary_xml)),
        };

        Ok(r)
    }

    pub fn add_file(&self, path: &std::path::Path) -> Result<()> {
        let rpm_file = std::fs::File::open(path)?;
        let mut buf_reader = std::io::BufReader::new(&rpm_file);
        let pkg = rpm::RPMPackage::parse(&mut buf_reader)
            .map_err(|err| anyhow!("{}", err.to_string()))?;

        {
            let mut primary_xml = self.primary_xml.lock().unwrap();
            let package_xml = quick_xml::se::to_string(
                &crate::repodata::xml::Package::of_rpm_package(&pkg, &rpm_file)?,
            )?;
            primary_xml.write_all(package_xml.as_bytes())?;
            primary_xml.write_all("\n\n\n".as_bytes())?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        let mut primary_xml = self.primary_xml.lock().unwrap();
        primary_xml.flush()?;

        let repodata_path = self.repo_path.join("new_repoadata");
        if repodata_path.exists() {
            std::fs::remove_dir_all(&repodata_path)
                .map_err(|err| anyhow!("Cannot remove old {:?}: {}", repodata_path, err))?;
        }
        let temp_path = self.tempdir.into_path();
        std::fs::rename(temp_path, repodata_path)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct Repodata {
    pub concurrency: usize,
}

impl Repodata {
    pub fn generate(&self, path: &std::path::Path) -> Result<()> {
        let files = path.read_dir()?.filter_map(|v| match v {
            Ok(v) => Some(v),
            Err(err) => {
                warn!("Cannot get entry in {:?}: {}", path, err);
                None
            }
        });
        let files: Vec<_> = files
            .filter(|v| v.file_name().to_string_lossy().ends_with(".rpm"))
            .collect();

        let state = State::new(path)?;

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.concurrency)
            .build()
            .unwrap();

        info!("Found {} RPM files", files.len());

        pool.install(|| {
            let _: Vec<_> = files
                .par_iter()
                .map(|v| {
                    let file_name = v.file_name();
                    let file_name_str = file_name.to_string_lossy();
                    info!("Processing file {}", file_name_str);
                    if let Err(err) = state.add_file(&v.path()) {
                        error!("Failed to process {:?}: {}", file_name_str, err)
                    }
                })
                .collect();
        });

        state.finish()?;

        Ok(())
    }
}
