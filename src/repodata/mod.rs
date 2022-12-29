pub mod xml;

use anyhow::{anyhow, Result};
use gzp::{
    deflate::Gzip,
    par::compress::{ParCompress, ParCompressBuilder},
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use slog::slog_o;
use slog_scope::{debug, error, info, warn};
use std::{
    collections::HashMap,
    io::{BufReader, Write},
    os::linux::fs::MetadataExt,
    sync::{Arc, Mutex},
};

pub struct State {
    _current_primary_xml_lock: Option<file_lock::FileLock>,
    current_packages: Arc<Mutex<HashMap<String, crate::repodata::xml::Package>>>,
    repo_path: std::path::PathBuf,
    tempdir: tempfile::TempDir,
    primary_xml: Arc<Mutex<crate::repodata::xml::Metadata>>,
}

impl State {
    fn repodata_path(repo_path: &std::path::Path) -> std::path::PathBuf {
        repo_path.join("repodata")
    }

    fn lock_current_primary_xml(
        repo_path: &std::path::Path,
    ) -> Result<Option<file_lock::FileLock>> {
        let current_primary_xml_path = Self::repodata_path(repo_path).join("primary.xml.gz");
        if current_primary_xml_path.exists() {
            info!("Setting exclusive lock to {:?}", current_primary_xml_path);
            Ok(Some(
                file_lock::FileLock::lock(
                    &current_primary_xml_path,
                    true,
                    file_lock::FileOptions::new().write(true),
                )
                .map_err(|err| anyhow!("Cannot lock {:?}: {}", current_primary_xml_path, err))?,
            ))
        } else {
            Ok(None)
        }
    }

    fn current_packages(
        repo_path: &std::path::Path,
    ) -> Result<HashMap<String, crate::repodata::xml::Package>> {
        let current_primary_xml_path = Self::repodata_path(repo_path).join("primary.xml.gz");
        info!(
            "Reading current metadata from {:?}",
            current_primary_xml_path
        );
        let file = std::fs::File::open(current_primary_xml_path)?;
        let reader = flate2::read::GzDecoder::new(file);
        let buf_reader = BufReader::new(reader);
        let list: crate::repodata::xml::Metadata = quick_xml::de::from_reader(buf_reader)?;
        info!("Got metadata for {} packages", list.package.len());
        let r = list
            .package
            .into_iter()
            .map(|p| (p.location.href.clone(), p))
            .collect();

        Ok(r)
    }

    pub fn new(repo_path: &std::path::Path) -> Result<Self> {
        let current_primary_xml = Self::lock_current_primary_xml(repo_path)?;
        let current_packages = match &current_primary_xml {
            Some(_) => match Self::current_packages(repo_path) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        "Will not use cached data due to read error of {:?}: {}",
                        Self::repodata_path(repo_path).join("primary.xml.gz"),
                        err
                    );
                    HashMap::new()
                }
            },
            None => HashMap::new(),
        };

        let tempdir = tempfile::Builder::new()
            .prefix(".repodata_")
            .tempdir_in(repo_path)?;

        info!("Will generate new repository index in {:?}", tempdir.path());

        let r = Self {
            tempdir,
            repo_path: repo_path.to_path_buf(),
            primary_xml: Arc::new(Mutex::new(crate::repodata::xml::Metadata::new())),
            _current_primary_xml_lock: current_primary_xml,
            current_packages: Arc::new(Mutex::new(current_packages)),
        };

        Ok(r)
    }

    pub fn add_file(&self, path: &std::path::Path) {
        let file_name = match path.file_name() {
            Some(v) => v.to_string_lossy().to_string(),
            None => {
                error!("Cannot calculate file name from path {:?}", path);
                return;
            }
        };

        let process = || {
            info!("Adding package");

            let metadata = path.metadata()?;

            let cached_package_record = {
                let mut current_packages = self.current_packages.lock().unwrap();
                match current_packages.remove(&file_name) {
                    Some(v) => {
                        if v.size.package == metadata.st_size()
                            && v.time.file == metadata.st_mtime()
                        {
                            debug!("Using cached package metadata");
                            Some(v)
                        } else {
                            None
                        }
                    }
                    None => None,
                }
            };

            let package = match cached_package_record {
                Some(v) => v,
                None => {
                    let mut rpm_file = std::fs::File::open(path)?;
                    let mut buf_reader = std::io::BufReader::new(&rpm_file);
                    let pkg = rpm::RPMPackage::parse(&mut buf_reader)
                        .map_err(|err| anyhow!("{}", err.to_string()))?;

                    let file_sha = match cached_package_record {
                        Some(v) => v.checksum.value.clone(),
                        None => crate::digest::file_sha128(&mut rpm_file)?,
                    };
                    crate::repodata::xml::Package::of_rpm_package(&pkg, path, &rpm_file, &file_sha)?
                }
            };

            {
                let mut primary_xml = self.primary_xml.lock().unwrap();
                primary_xml.add_package(package);
            }
            let r: anyhow::Result<()> = Ok(());
            r
        };

        slog_scope::scope(
            &slog_scope::logger().new(slog_o!("package" => file_name.clone())),
            || {
                if let Err(err) = process() {
                    error!("Failed to process: {}", err)
                }
            },
        )
    }

    fn finish_primary_xml(&self) -> Result<()> {
        let primary_xml = std::fs::File::create(self.tempdir.path().join("primary.xml.gz"))?;
        let mut primary_xml: ParCompress<Gzip> = ParCompressBuilder::new().from_writer(primary_xml);

        let metadata = self.primary_xml.lock().unwrap();

        primary_xml.write_all(quick_xml::se::to_string(&*metadata)?.as_bytes())?;
        primary_xml.flush()?;

        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.finish_primary_xml()?;

        let repodata_path = Self::repodata_path(&self.repo_path);
        if repodata_path.exists() {
            info!("Removing old {:?}", repodata_path);
            std::fs::remove_dir_all(&repodata_path)
                .map_err(|err| anyhow!("Cannot remove old {:?}: {}", repodata_path, err))?;
        }
        let temp_path = self.tempdir.into_path();
        info!("Renaming {:?} to {:?}", temp_path, repodata_path);
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
                .map(|v| state.add_file(&v.path()))
                .collect();
        });

        state.finish()?;

        Ok(())
    }
}
