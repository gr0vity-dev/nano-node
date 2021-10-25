use anyhow::Result;

pub trait Stream {
    fn write_u8(&mut self, value: u8) -> anyhow::Result<()>;
    fn write_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()>;
    fn read_u8(&mut self) -> anyhow::Result<u8>;
    fn read_bytes(&mut self, buffer: &mut [u8], len: usize) -> anyhow::Result<()>;
}

pub trait Blake2b {
    fn init(&mut self, outlen: usize) -> Result<()>;
    fn update(&mut self, bytes: &[u8]) -> Result<()>;
    fn finalize(&mut self, out: &mut [u8]) -> Result<()>;
}
