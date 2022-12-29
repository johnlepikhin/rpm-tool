use std::io::{Read, Seek, SeekFrom};

use anyhow::Result;

pub fn file_sha128(file: &mut std::fs::File) -> Result<String> {
    use crypto::digest::Digest;
    use crypto::sha1::Sha1;

    file.seek(SeekFrom::Start(0))?;

    let mut hasher = Sha1::new();
    let mut buffer = [0; 1024];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.input(&buffer[..count]);
    }

    Ok(hasher.result_str())
}

pub fn path_sha128(path: &std::path::Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    file_sha128(&mut file)
}

pub fn str_sha128(str: &str) -> String {
    use crypto::digest::Digest;
    use crypto::sha1::Sha1;

    let mut hasher = Sha1::new();
    hasher.input_str(str);

    hasher.result_str()
}
