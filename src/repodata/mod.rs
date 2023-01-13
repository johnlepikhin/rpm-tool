mod filelists;
pub mod primary;
mod repomd;

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
    io::Write,
    os::linux::fs::MetadataExt,
    rc::Rc,
    sync::{Arc, Mutex},
};

#[derive(Serialize, Deserialize)]
pub struct RepodataConfig {
    pub concurrency: usize,
    #[serde(with = "serde_regex")]
    pub useful_files: regex::Regex,
}

#[derive(Serialize, Deserialize)]
pub struct RepodataOptions {
    pub generate_fileslists: bool,
    pub path: std::path::PathBuf,
}

struct State<'a> {
    config: &'a RepodataConfig,
    options: &'a RepodataOptions,
    _current_primary_xml_lock: Option<file_lock::FileLock>,
    current_packages: Arc<Mutex<HashMap<String, crate::repodata::primary::Package>>>,
    current_fileslist: Arc<Mutex<HashMap<String, crate::repodata::filelists::Package>>>,
    tempdir: tempfile::TempDir,
    primary_xml: Arc<Mutex<crate::repodata::primary::Primary>>,
    fileslist: Arc<Mutex<crate::repodata::filelists::Filelists>>,
}

impl<'a> State<'a> {
    fn repodata_path(&self) -> std::path::PathBuf {
        self.options.path.join("repodata")
    }

    fn lock_current_primary_xml(path: &std::path::Path) -> Result<Option<file_lock::FileLock>> {
        let current_primary_xml_path = path.join("repodata").join("primary.xml.gz");
        if current_primary_xml_path.exists() {
            info!("Setting exclusive lock on {:?}", current_primary_xml_path);
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
        path: &std::path::Path,
    ) -> Result<HashMap<String, crate::repodata::primary::Package>> {
        let path = path.join("repodata").join("primary.xml.gz");
        let primary = crate::repodata::primary::Primary::read(&path)?;
        info!(
            "Got primary metadata for {} packages",
            primary.package.len()
        );
        let r = primary
            .package
            .into_iter()
            .map(|p| (p.location.href.clone(), p))
            .collect();

        Ok(r)
    }

    fn current_fileslist(
        path: &std::path::Path,
    ) -> Result<HashMap<String, crate::repodata::filelists::Package>> {
        let path = path.join("repodata").join("fileslists.xml.gz");
        let fileslists = crate::repodata::filelists::Filelists::read(&path)?;
        info!("Got fileslists for {} packages", fileslists.package.len());
        let r = fileslists
            .package
            .into_iter()
            .map(|p| (p.pkgid.clone(), p))
            .collect();

        Ok(r)
    }

    pub fn new(config: &'a RepodataConfig, options: &'a RepodataOptions) -> Result<Self> {
        let current_primary_xml = Self::lock_current_primary_xml(&options.path)?;
        let current_packages = match &current_primary_xml {
            Some(_) => match Self::current_packages(&options.path) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        "Will not use primary cached data due to read error of primary.xml.gz: {}",
                        err
                    );
                    HashMap::new()
                }
            },
            None => HashMap::new(),
        };

        let tempdir = tempfile::Builder::new()
            .prefix(".repodata_")
            .tempdir_in(&options.path)?;

        let current_fileslist = if options.generate_fileslists {
            match Self::current_fileslist(&options.path) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        "Will not use fileslists cached data due to read error of fileslists.xml.gz: {}",
                        err
                    );
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        info!("Will generate new repository index in {:?}", tempdir.path());

        let r = Self {
            tempdir,
            primary_xml: Arc::new(Mutex::new(crate::repodata::primary::Primary::new())),
            fileslist: Arc::new(Mutex::new(crate::repodata::filelists::Filelists::new())),
            _current_primary_xml_lock: current_primary_xml,
            current_packages: Arc::new(Mutex::new(current_packages)),
            current_fileslist: Arc::new(Mutex::new(current_fileslist)),
            options,
            config,
        };

        Ok(r)
    }

    fn read_rpm(path: &std::path::Path) -> Result<rpm::RPMPackage> {
        let rpm_file = std::fs::File::open(path)?;
        let mut buf_reader = std::io::BufReader::new(&rpm_file);
        rpm::RPMPackage::parse(&mut buf_reader).map_err(|err| anyhow!("{}", err.to_string()))
    }

    pub fn add_file(&self, path: &std::path::Path, file_name: String) -> Result<()> {
        info!("Adding package");

        let path_clone = path.to_path_buf();
        let lazy_file_sha =
            crate::lazy_result::LazyResult::new(move || crate::digest::path_sha128(&path_clone));
        let path_clone = path.to_path_buf();
        let lazy_rpm_head =
            crate::lazy_result::LazyResult::new(move || Self::read_rpm(&path_clone));
        let path_clone = path.to_path_buf();
        let lazy_metadata: crate::lazy_result::LazyResult<_, anyhow::Error> =
            crate::lazy_result::LazyResult::new(move || {
                let r = path_clone.metadata()?;
                Ok(r)
            });

        let cached_package_record = {
            let mut current_packages = self.current_packages.lock().unwrap();
            match current_packages.remove(&file_name) {
                Some(v) => {
                    let metadata = lazy_metadata.get()?;
                    if v.size.package == metadata.st_size() && v.time.file == metadata.st_mtime() {
                        debug!("Using cached package metadata");
                        Some(v)
                    } else {
                        None
                    }
                }
                None => None,
            }
        };

        let (package, is_new_record) = match cached_package_record {
            Some(v) => (v, false),
            None => {
                let file_sha = match cached_package_record {
                    Some(v) => Rc::new(v.checksum.value),
                    None => lazy_file_sha.get()?,
                };
                let package = crate::repodata::primary::Package::of_rpm_package(
                    &*lazy_rpm_head.get()?,
                    path,
                    &file_sha,
                    &self.config.useful_files,
                )?;
                (package, true)
            }
        };

        let sha = package.checksum.value.clone();

        {
            let mut primary_xml = self.primary_xml.lock().unwrap();
            primary_xml.add_package(package);
        }

        if self.options.generate_fileslists {
            let package = if is_new_record {
                crate::repodata::filelists::Package::of_rpm_package(
                    &*lazy_rpm_head.get()?,
                    &lazy_file_sha.get()?,
                )?
            } else {
                let mut cache = self.current_fileslist.lock().unwrap();
                match cache.remove(&sha) {
                    Some(v) => v,
                    None => crate::repodata::filelists::Package::of_rpm_package(
                        &*lazy_rpm_head.get()?,
                        &lazy_file_sha.get()?,
                    )?,
                }
            };
            let mut fileslist = self.fileslist.lock().unwrap();
            fileslist.add_package(package)
        }

        let r: anyhow::Result<()> = Ok(());
        r
    }

    fn finish_xml<T>(
        &self,
        filename: &str,
        data: &T,
        data_type: crate::repodata::repomd::DataType,
    ) -> Result<crate::repodata::repomd::Data>
    where
        T: Serialize,
    {
        let gz_filename = format!("{}.xml.gz", filename);
        let path = self.tempdir.path().join(&gz_filename);

        info!("Generating {gz_filename}");

        let xml_str = {
            let file = std::fs::File::create(&path)?;
            let mut gz_file: ParCompress<Gzip> = ParCompressBuilder::new().from_writer(file);

            let primary_xml_str = quick_xml::se::to_string(data)?;

            gz_file.write_all(primary_xml_str.as_bytes())?;
            gz_file.flush()?;

            primary_xml_str
        };

        let checksum = crate::digest::path_sha128(&path)?;

        let metadata = path.metadata()?;

        let open_checksum = crate::digest::str_sha128(&xml_str);
        let open_size = xml_str.len();

        let r = crate::repodata::repomd::Data {
            type_: data_type,
            checksum: crate::repodata::repomd::Checksum::new(checksum),
            open_checksum: crate::repodata::repomd::Checksum::new(open_checksum),
            location: crate::repodata::repomd::Location::new(format!("repodata/{}", gz_filename)),
            timestamp: metadata.st_mtime(),
            size: metadata.st_size(),
            open_size,
        };

        Ok(r)
    }

    fn finish_repomd(&self, repomd: crate::repodata::repomd::Repomd) -> Result<()> {
        let filename = "repomd.xml";
        info!("Generating {filename}");
        let path = self.tempdir.path().join(filename);
        let mut file = std::fs::File::create(&path)?;
        file.write_all(quick_xml::se::to_string(&repomd)?.as_bytes())?;

        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        let mut repomd = crate::repodata::repomd::Repomd::new();

        let metadata = self.primary_xml.lock().unwrap();
        repomd.add_data(self.finish_xml(
            "primary",
            &*metadata,
            crate::repodata::repomd::DataType::Primary,
        )?);

        if self.options.generate_fileslists {
            let metadata = self.fileslist.lock().unwrap();
            repomd.add_data(self.finish_xml(
                "fileslists",
                &*metadata,
                crate::repodata::repomd::DataType::Filelists,
            )?);
        }

        self.finish_repomd(repomd)?;

        let repodata_path = self.repodata_path();
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

pub struct Repodata<'a> {
    pub config: &'a RepodataConfig,
    pub options: RepodataOptions,
}

impl<'a> Repodata<'a> {
    pub fn generate(&self) -> Result<()> {
        let files = self.options.path.read_dir()?.filter_map(|v| match v {
            Ok(v) => Some(v),
            Err(err) => {
                warn!("Cannot get entry in {:?}: {}", self.options.path, err);
                None
            }
        });
        let files: Vec<_> = files
            .filter(|v| v.file_name().to_string_lossy().ends_with(".rpm"))
            .collect();

        let state = State::new(self.config, &self.options)?;

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.config.concurrency)
            .build()
            .unwrap();

        info!("Found {} RPM files", files.len());

        pool.install(|| {
            let _: Vec<_> = files
                .par_iter()
                .map(|v| {
                    let file_name = match v.path().file_name() {
                        Some(v) => v.to_string_lossy().to_string(),
                        None => {
                            error!(
                                "Cannot calculate file name from path {:?}",
                                self.options.path
                            );
                            return;
                        }
                    };
                    slog_scope::scope(
                        &slog_scope::logger().new(slog_o!("package" => file_name.clone())),
                        || {
                            if let Err(err) = state.add_file(&v.path(), file_name) {
                                error!("Failed to process: {}", err);
                            }
                        },
                    )
                })
                .collect();
        });

        state.finish()?;

        Ok(())
    }
}
