mod filelists;
pub mod primary;
mod repomd;

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use slog::slog_o;
use slog_scope::{debug, error, info, trace, warn};
use std::{
    collections::{HashMap, HashSet},
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
    _current_repomd_xml_lock: Option<file_lock::FileLock>,
    current_packages: Arc<Mutex<HashMap<std::path::PathBuf, crate::repodata::primary::Package>>>,
    current_fileslist: Arc<Mutex<HashMap<String, crate::repodata::filelists::Package>>>,
    tempdir: tempfile::TempDir,
    primary_xml: Arc<Mutex<crate::repodata::primary::Primary>>,
    fileslist: Arc<Mutex<crate::repodata::filelists::Filelists>>,
}

impl<'a> State<'a> {
    fn empty_new(
        config: &'a RepodataConfig,
        options: &'a RepodataOptions,
        current_repomd_xml_lock: Option<file_lock::FileLock>,
    ) -> Result<Self> {
        let tempdir = tempfile::Builder::new()
            .prefix(".repodata_")
            .tempdir_in(&options.path)?;

        Ok(Self {
            tempdir,
            primary_xml: Arc::new(Mutex::new(crate::repodata::primary::Primary::new())),
            fileslist: Arc::new(Mutex::new(crate::repodata::filelists::Filelists::new())),
            _current_repomd_xml_lock: current_repomd_xml_lock,
            current_packages: Arc::new(Mutex::new(HashMap::new())),
            current_fileslist: Arc::new(Mutex::new(HashMap::new())),
            options,
            config,
        })
    }

    fn repodata_path(&self) -> std::path::PathBuf {
        self.options.path.join("repodata")
    }

    fn lock_current_repomd_xml(path: &std::path::Path) -> Result<Option<file_lock::FileLock>> {
        let xml_path = path.join("repodata").join("repomd.xml");
        if xml_path.exists() {
            info!("Setting exclusive lock on {:?}", xml_path);
            Ok(Some(
                file_lock::FileLock::lock(
                    &xml_path,
                    true,
                    file_lock::FileOptions::new().write(true),
                )
                .map_err(|err| anyhow!("Cannot lock {:?}: {}", xml_path, err))?,
            ))
        } else {
            Ok(None)
        }
    }

    fn current_repomd(path: &std::path::Path) -> Result<crate::repodata::repomd::Repomd> {
        let path = path.join("repodata").join("repomd.xml");
        let xml = crate::repodata::repomd::Repomd::read(&path)?;
        info!("Read repomd with {} records", xml.data.len());
        Ok(xml)
    }

    fn current_packages(
        path: &std::path::Path,
    ) -> Result<HashMap<std::path::PathBuf, crate::repodata::primary::Package>> {
        let primary = crate::repodata::primary::Primary::read(path)?;
        info!(
            "Got primary metadata for {} packages",
            primary.package.len()
        );
        let r = primary
            .package
            .into_iter()
            .map(|p| (std::path::Path::new(&p.location.href).to_path_buf(), p))
            .collect();

        Ok(r)
    }

    fn current_fileslist(
        path: &std::path::Path,
    ) -> Result<HashMap<String, crate::repodata::filelists::Package>> {
        let fileslists = crate::repodata::filelists::Filelists::read(path)?;
        info!("Got fileslists for {} packages", fileslists.package.len());
        let r = fileslists
            .package
            .into_iter()
            .map(|p| (p.pkgid.clone(), p))
            .collect();

        Ok(r)
    }

    pub fn new(config: &'a RepodataConfig, options: &'a RepodataOptions) -> Result<Self> {
        let current_repomd_xml = Self::lock_current_repomd_xml(&options.path)?;
        let current_repomd = match &current_repomd_xml {
            Some(_) => match Self::current_repomd(&options.path) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        "Will not use cached data due to read error of repomd.xml: {}",
                        err
                    );
                    return Self::empty_new(config, options, None);
                }
            },
            None => return Self::empty_new(config, options, None),
        };

        let current_packages = if let Some(primary_xml_md) = current_repomd
            .data
            .iter()
            .find(|elt| elt.type_ == crate::repodata::repomd::DataType::Primary)
        {
            let location = &primary_xml_md.location.href;
            match Self::current_packages(&options.path.join(location)) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        "Will not use primary cached data due to read error of {:?}: {}",
                        location, err
                    );
                    HashMap::new()
                }
            }
        } else {
            warn!("No 'primary' record in repomd.xml");
            HashMap::new()
        };

        let tempdir = tempfile::Builder::new()
            .prefix(".repodata_")
            .tempdir_in(&options.path)?;

        let current_fileslist = if options.generate_fileslists {
            if let Some(fileslists_xml_md) = current_repomd
                .data
                .iter()
                .find(|elt| elt.type_ == crate::repodata::repomd::DataType::Filelists)
            {
                let location = &fileslists_xml_md.location.href;
                match Self::current_fileslist(&options.path.join(location)) {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(
                            "Will not use fileslists cached data due to read error of {:?}: {}",
                            location, err
                        );
                        HashMap::new()
                    }
                }
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        info!("Will generate new repository index in {:?}", tempdir.path());

        let r = Self {
            tempdir,
            primary_xml: Arc::new(Mutex::new(crate::repodata::primary::Primary::new())),
            fileslist: Arc::new(Mutex::new(crate::repodata::filelists::Filelists::new())),
            _current_repomd_xml_lock: current_repomd_xml,
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

    pub fn add_file(&self, path: &std::path::Path, relative_path: &std::path::Path) -> Result<()> {
        debug!("Adding package");

        let path_clone = path.to_path_buf();
        let lazy_file_sha = crate::lazy_result::LazyResult::new(move || {
            trace!("Calculating SHA128");
            let r = crate::digest::path_sha128(&path_clone)
                .map_err(|err| anyhow!("Calculate file SHA1 for {:?}: {}", path_clone, err));
            trace!("Done calculating SHA128");
            r
        });
        let path_clone = path.to_path_buf();
        let lazy_rpm_head = crate::lazy_result::LazyResult::new(move || {
            trace!("Reading RPM header");
            let r = Self::read_rpm(&path_clone)
                .map_err(|err| anyhow!("Read RPM header from {:?}: {}", path_clone, err));
            trace!("Done reading RPM header");
            r
        });
        let path_clone = path.to_path_buf();
        let lazy_metadata: crate::lazy_result::LazyResult<_, anyhow::Error> =
            crate::lazy_result::LazyResult::new(move || {
                trace!("Reading RPM metadata");
                let r = path_clone
                    .metadata()
                    .map_err(|err| anyhow!("Read metadata for {:?}: {}", path_clone, err))?;
                trace!("Done reading RPM metadata");
                Ok(r)
            });

        let cached_package_record = {
            let mut current_packages = self.current_packages.lock().unwrap();
            match current_packages.remove(relative_path) {
                Some(v) => {
                    let metadata = lazy_metadata.get()?;
                    if v.size.package == metadata.st_size() && v.time.file == metadata.st_mtime() {
                        debug!("st_size and st_mtime are the same, using cached package metadata");
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
                info!("No cached primary metadata found, calculating SHA of package");
                let file_sha = match cached_package_record {
                    Some(v) => Rc::new(v.checksum.value),
                    None => lazy_file_sha.get()?,
                };
                let package = crate::repodata::primary::Package::of_rpm_package(
                    &*lazy_rpm_head.get()?,
                    path,
                    relative_path,
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
                    None => {
                        trace!("No cached fileslist, will generate new record from RPM headers");
                        crate::repodata::filelists::Package::of_rpm_package(
                            &*lazy_rpm_head.get()?,
                            &lazy_file_sha.get()?,
                        )?
                    }
                }
            };
            let mut fileslist = self.fileslist.lock().unwrap();
            fileslist.add_package(package)
        }

        let r: anyhow::Result<()> = Ok(());
        r
    }

    #[cfg(feature = "parallel-zip")]
    fn parallel_zip(path: &std::path::Path, str: &str) -> Result<()> {
        use gzp::{
            deflate::Gzip,
            par::compress::{ParCompress, ParCompressBuilder},
        };

        let file = std::fs::File::create(&path)?;
        let mut gz_file: ParCompress<Gzip> = ParCompressBuilder::new().from_writer(file);

        gz_file.write_all(str.as_bytes())?;
        gz_file.flush()?;

        Ok(())
    }

    #[cfg(not(feature = "parallel-zip"))]
    fn single_threaded_zip(path: &std::path::Path, str: &str) -> Result<()> {
        let file = std::fs::File::create(&path)?;
        let mut writer = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        writer.write_all(str.as_bytes())?;
        Ok(())
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
            let primary_xml_str = quick_xml::se::to_string(data)?;

            #[cfg(feature = "parallel-zip")]
            Self::parallel_zip(&path, &primary_xml_str)?;

            #[cfg(not(feature = "parallel-zip"))]
            Self::single_threaded_zip(&path, &primary_xml_str)?;

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

    pub fn restore_current(&self) {
        let mut current_packages = self.current_packages.lock().unwrap();
        let mut primary_xml = self.primary_xml.lock().unwrap();
        for (_, package) in current_packages.drain() {
            primary_xml.add_package(package);
        }

        let mut current_fileslists = self.current_fileslist.lock().unwrap();
        let mut fileslists = self.fileslist.lock().unwrap();
        for (_, package) in current_fileslists.drain() {
            fileslists.add_package(package);
        }
    }

    pub fn drain_files(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Vec<crate::repodata::primary::Package> {
        let mut primary_xml = self.primary_xml.lock().unwrap();

        let removed_packages: Vec<_> = primary_xml.drain_filter(|package| {
            !paths.contains(&std::path::PathBuf::from(&package.location.href))
        });

        let removed_ids: HashSet<_> = removed_packages
            .iter()
            .map(|package| package.checksum.value.clone())
            .collect();

        let mut fileslists = self.fileslist.lock().unwrap();
        let _ = fileslists.drain_filter(|package| !removed_ids.contains(&package.pkgid));

        removed_packages
    }
}

pub struct Repodata<'a> {
    pub config: &'a RepodataConfig,
    pub options: RepodataOptions,
}

impl<'a> Repodata<'a> {
    fn register_files_list(&self, state: State, files: &[std::path::PathBuf]) -> Result<()> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(self.config.concurrency)
            .build()
            .unwrap();

        pool.install(|| {
            let _: Vec<_> = files
                .par_iter()
                .map(|v| {
                    let relative_path = match v.strip_prefix(&self.options.path) {
                        Ok(v) => v,
                        Err(err) => {
                            error!(
                                "Cannot strip base repo path from file path {:?}: {}",
                                self.options.path, err
                            );
                            return;
                        }
                    };
                    slog_scope::scope(
                        &slog_scope::logger()
                            .new(slog_o!("package" => relative_path.to_string_lossy().to_string())),
                        || {
                            if let Err(err) = state.add_file(v, relative_path) {
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
    pub fn generate(&self) -> Result<()> {
        let mut files = Vec::new();
        files.reserve(50000);
        for elt in walkdir::WalkDir::new(&self.options.path).same_file_system(true) {
            let elt = match elt {
                Ok(v) => v,
                Err(err) => {
                    warn!("Cannot get entry in {:?}: {}", self.options.path, err);
                    continue;
                }
            };
            if !elt
                .file_name()
                .to_str()
                .map(|v| v.to_lowercase().ends_with(".rpm"))
                .unwrap_or(false)
            {
                continue;
            }
            match elt.metadata() {
                Ok(v) => {
                    if !v.is_file() {
                        continue;
                    }
                }
                Err(err) => {
                    warn!("Cannot read entry metadata {:?}: {}", elt.path(), err);
                    continue;
                }
            }

            let path = elt.path().to_owned();
            debug!("Found RPM file {:?}", path);
            files.push(path)
        }

        info!("Found {} RPM files", files.len());

        let state = State::new(self.config, &self.options)?;

        self.register_files_list(state, &files)
    }

    pub fn add_files(&self, files: &[std::path::PathBuf]) -> Result<()> {
        let files: Vec<_> = files
            .iter()
            .filter(|path| {
                let full_path = self.options.path.join(path);
                if !full_path.exists() {
                    warn!("File {:?} not found, skipping", path);
                    false
                } else {
                    match path.file_name() {
                        None => {
                            warn!("Path {:?} does not contain file name, skipping", path);
                            false
                        }
                        Some(file_name) => {
                            if !file_name.to_string_lossy().ends_with(".rpm") {
                                warn!(
                                    "File {:?} does not seem to have .rpm extension, skipping",
                                    path
                                );
                                false
                            } else {
                                true
                            }
                        }
                    }
                }
            })
            .map(|v| v.to_owned())
            .collect();

        info!("Will add {} RPM files", files.len());

        let state = State::new(self.config, &self.options)?;
        state.restore_current();

        let removed_packages = state.drain_files(&files);

        info!(
            "Removed {} records from current index about packages to be re-added",
            removed_packages.len()
        );

        self.register_files_list(
            state,
            &files
                .into_iter()
                .map(|v| self.options.path.join(v))
                .collect::<Vec<_>>(),
        )
    }

    pub fn validate(&self) -> Result<()> {
        let _state = State::new(self.config, &self.options)?;
        Ok(())
    }
}
