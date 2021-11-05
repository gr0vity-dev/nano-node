use std::cell::Ref;

use crate::{
    numbers::{
        from_string_hex, sign_message, to_string_hex, Account, BlockHash, PublicKey, RawKey,
        Signature,
    },
    utils::{Blake2b, PropertyTreeReader, PropertyTreeWriter, RustBlake2b, Stream},
};
use anyhow::Result;

use super::{BlockHashFactory, LazyBlockHash};

#[derive(Clone, PartialEq, Eq)]
pub struct OpenHashables {
    pub source: BlockHash,
    pub representative: Account,
    pub account: Account,
}

impl OpenHashables {
    const fn serialized_size() -> usize {
        BlockHash::serialized_size() + Account::serialized_size() + Account::serialized_size()
    }
}

#[derive(Clone)]
pub struct OpenBlock {
    pub work: u64,
    pub signature: Signature,
    pub hashables: OpenHashables,
    pub hash: LazyBlockHash,
}

impl OpenBlock {
    pub fn new(
        source: BlockHash,
        representative: Account,
        account: Account,
        prv_key: &RawKey,
        pub_key: &PublicKey,
        work: u64,
    ) -> Result<Self> {
        let mut result = Self {
            work,
            signature: Signature::new(),
            hashables: OpenHashables {
                source,
                representative,
                account,
            },
            hash: LazyBlockHash::new(),
        };

        let signature = sign_message(&prv_key, &pub_key, result.hash().as_bytes())?;
        result.signature = signature;
        Ok(result)
    }

    pub const fn serialized_size() -> usize {
        OpenHashables::serialized_size() + Signature::serialized_size() + std::mem::size_of::<u64>()
    }

    pub fn hash(&self) -> Ref<BlockHash> {
        self.hash.hash(self)
    }

    pub fn hash_hashables(&self, blake2b: &mut impl Blake2b) -> Result<()> {
        blake2b.update(&self.hashables.source.to_bytes())?;
        blake2b.update(&self.hashables.representative.to_bytes())?;
        blake2b.update(&self.hashables.account.to_bytes())?;
        Ok(())
    }

    pub fn serialize(&self, stream: &mut impl Stream) -> Result<()> {
        self.hashables.source.serialize(stream)?;
        self.hashables.representative.serialize(stream)?;
        self.hashables.account.serialize(stream)?;
        self.signature.serialize(stream)?;
        stream.write_bytes(&self.work.to_be_bytes())?;
        Ok(())
    }

    pub fn deserialize(&mut self, stream: &mut impl Stream) -> Result<()> {
        self.hashables.source = BlockHash::deserialize(stream)?;
        self.hashables.representative.deserialize(stream)?;
        self.hashables.account.deserialize(stream)?;
        self.signature = Signature::deserialize(stream)?;
        let mut work_bytes = [0u8; 8];
        stream.read_bytes(&mut work_bytes, 8)?;
        self.work = u64::from_be_bytes(work_bytes);
        Ok(())
    }

    pub fn serialize_json(&self, writer: &mut impl PropertyTreeWriter) -> Result<()> {
        writer.put_string("type", "open")?;
        writer.put_string("source", &self.hashables.source.encode_hex())?;
        writer.put_string(
            "representative",
            &self.hashables.representative.encode_account(),
        )?;
        writer.put_string("account", &self.hashables.account.encode_account())?;
        writer.put_string("work", &to_string_hex(self.work))?;
        writer.put_string("signature", &self.signature.encode_hex())?;
        Ok(())
    }

    pub fn deserialize_json(reader: &impl PropertyTreeReader) -> Result<Self> {
        let source = BlockHash::decode_hex(reader.get_string("source")?)?;
        let representative = Account::decode_account(reader.get_string("representative")?)?;
        let account = Account::decode_account(reader.get_string("account")?)?;
        let work = from_string_hex(reader.get_string("work")?)?;
        let signature = Signature::decode_hex(reader.get_string("signature")?)?;
        Ok(OpenBlock {
            work,
            signature,
            hashables: OpenHashables {
                source,
                representative,
                account,
            },
            hash: LazyBlockHash::new(),
        })
    }
}

impl PartialEq for OpenBlock {
    fn eq(&self, other: &Self) -> bool {
        self.work == other.work
            && self.signature == other.signature
            && self.hashables == other.hashables
    }
}

impl Eq for OpenBlock {}

impl BlockHashFactory for OpenBlock {
    fn hash(&self) -> BlockHash {
        let mut blake = RustBlake2b::new();
        blake.init(32).unwrap();
        self.hash_hashables(&mut blake).unwrap();
        let mut result = [0u8; 32];
        blake.finalize(&mut result).unwrap();
        BlockHash::from_bytes(result)
    }
}
