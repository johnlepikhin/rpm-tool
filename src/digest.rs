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
