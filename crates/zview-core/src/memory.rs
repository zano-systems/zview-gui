//! Target operations shared by live acquisition and strict replay.
use anyhow::Result;

pub(crate) trait Memory {
    fn connect(&mut self) -> Result<()>;
    fn begin_batch(&mut self) -> Result<()>;
    fn end_batch(&mut self) -> Result<()>;
    fn read8(&mut self, at: u32, amount: u32) -> Result<Vec<u8>>;
    fn read32(&mut self, at: u32, amount: u32) -> Result<Vec<u32>>;
    fn read64(&mut self, at: u32) -> Result<u64>;
    fn read_bytes(&mut self, at: u32, amount: u32) -> Result<Vec<u8>>;

    // Only legacy reads whose very presence was not described in the header
    // use these hooks. A present read with a malformed result is still fatal.
    fn optional_word(&mut self, at: u32) -> Result<Option<u32>> {
        self.word(at).map(Some)
    }
    fn optional_byte(&mut self, at: u32) -> Result<Option<u8>> {
        Ok(Some(self.read8(at, 1)?[0]))
    }

    fn word(&mut self, at: u32) -> Result<u32> {
        Ok(self.read32(at, 1)?[0])
    }
}
