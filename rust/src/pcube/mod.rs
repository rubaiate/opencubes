use std::{
    fs::File,
    io::{ErrorKind, Read, Write},
    path::Path,
};

mod raw_pcube;
pub use raw_pcube::RawPCube;

mod traits;

mod compression;
pub use compression::Compression;
use compression::{Reader, Writer};

const MAGIC: [u8; 4] = [0xCB, 0xEC, 0xCB, 0xEC];

pub struct PolyCubeFile<T = File>
where
    T: Read,
{
    had_error: bool,
    input: Reader<T>,
    len: Option<usize>,
    cubes_read: usize,
    cubes_are_canonical: bool,
}

impl<T> Iterator for PolyCubeFile<T>
where
    T: Read,
{
    type Item = std::io::Result<RawPCube>;

    fn size_hint(&self) -> (usize, Option<usize>) {
        if let Some(len) = self.len {
            (0, Some(len))
        } else {
            (0, None)
        }
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.had_error {
            return None;
        }

        let next_cube = RawPCube::unpack(&mut self.input);

        let next_cube = match (next_cube, self.len) {
            (Err(_), None) => return None,
            (Err(e), Some(expected)) => {
                if expected == self.cubes_read {
                    return None;
                } else {
                    self.had_error = true;
                    let msg = format!(
                        "Expected {expected} cubes, but failed to read after {} cubes. Error: {e}",
                        self.cubes_read
                    );
                    return Some(Err(std::io::Error::new(ErrorKind::InvalidData, msg)));
                }
            }
            (Ok(c), _) => c,
        };

        self.cubes_read += 1;

        Some(Ok(next_cube))
    }
}

impl<T> PolyCubeFile<T>
where
    T: Read,
{
    pub fn compression(&self) -> Compression {
        match self.input {
            Reader::Uncompressed(_) => Compression::None,
            Reader::Gzip(_) => Compression::Gzip,
        }
    }

    pub fn len(&self) -> Option<usize> {
        self.len
    }

    pub fn canonical(&self) -> bool {
        self.cubes_are_canonical
    }

    pub fn new(mut input: T) -> std::io::Result<Self> {
        let mut magic = [0u8; 4];
        input.read_exact(&mut magic)?;

        if magic != MAGIC {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "File magic was incorrect.",
            ));
        }

        let mut header = [0u8; 2];
        input.read_exact(&mut header)?;

        let [orientation, compression] = header;
        let canonicalized = orientation != 0;

        let mut cube_count: u64 = 0;
        let mut shift = 0;
        loop {
            let mut next_byte = [0u8; 1];
            input.read_exact(&mut next_byte)?;

            let [next_byte] = next_byte;

            cube_count |= ((next_byte & 0x7F) as u64) << shift;

            shift += 7;
            if shift > 64 {
                panic!("Cannot load possibly more than u64 cubes...");
            }

            if next_byte & 0x80 == 0 {
                break;
            }
        }

        let len = if cube_count == 0 {
            None
        } else {
            Some(cube_count as usize)
        };

        let input = match Compression::try_from(compression) {
            Ok(c) => Reader::new(c, input),
            Err(_) => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!("Unsupported compression type {compression}"),
                ))
            }
        };

        Ok(Self {
            input,
            len,
            cubes_read: 0,
            cubes_are_canonical: canonicalized,
            had_error: false,
        })
    }
}

impl PolyCubeFile {
    pub fn new_file(p: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = std::fs::File::open(p.as_ref())?;
        Self::new(file)
    }

    pub fn write<C, I, W>(
        mut cubes: I,
        is_canonical: bool,
        compression: Compression,
        mut write: W,
    ) -> std::io::Result<()>
    where
        I: Iterator<Item = C>,
        C: Into<RawPCube>,
        W: Write,
    {
        write.write_all(&MAGIC)?;

        let compression_val = compression.into();
        let orientation_val = if is_canonical { 1 } else { 0 };

        write.write_all(&[orientation_val, compression_val])?;

        let mut cube_count = 0;
        let (_, max) = cubes.size_hint();

        if let Some(max) = max {
            cube_count = max;
        }

        let mut ran_once = false;
        while cube_count > 0 || !ran_once {
            ran_once = true;
            let mut next_byte = (cube_count as u8) & 0x7F;
            cube_count >>= 7;

            if cube_count > 0 {
                next_byte |= 0x80;
            }

            write.write_all(&[next_byte])?;
        }

        let mut writer = Writer::new(compression, write);

        if let Some(e) = cubes.find_map(|v| v.into().pack(&mut writer).err()) {
            return Err(e);
        }

        Ok(())
    }
}
