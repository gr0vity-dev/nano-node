use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Mutex,
        mpsc::{self, SyncSender},
    },
    thread::{self, available_parallelism},
};

use ed25519_dalek_blake2b::SignatureError;
use rsnano_ledger::LedgerConstants;
use rsnano_nullable_lmdb::LmdbEnvironmentFactory;
use rsnano_store_lmdb::{EnvironmentFlags, EnvironmentOptions, LmdbBlockStore, LmdbLedgerReadTxn};
use rsnano_types::{Epochs, PublicKey, SavedBlock, Signature};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("usage: signature-checker LEDGER_FILE_PATH");
        return;
    }

    let ledger_file = PathBuf::from(&args[1]);
    check_ledger_file(ledger_file);
}

fn check_ledger_file(ledger_file: impl Into<PathBuf>) {
    let env_options = EnvironmentOptions {
        max_dbs: 128,
        map_size: 256 * 1024 * 1024 * 1024,
        flags: EnvironmentFlags::NO_SUB_DIR
            | EnvironmentFlags::NO_TLS
            | EnvironmentFlags::NO_MEM_INIT,
        path: ledger_file.into(),
    };
    let env = LmdbEnvironmentFactory::default()
        .create(env_options)
        .unwrap();
    let block_store = LmdbBlockStore::new(&env).unwrap();

    let tx = LmdbLedgerReadTxn::new(env.begin_read());
    let total_blocks = block_store.count(&tx);
    let mut checked: u64 = 0;
    let problematic = Mutex::new(Vec::new());
    let epochs = LedgerConstants::live().epochs;
    let cpus = available_parallelism().unwrap();

    thread::scope(|s| {
        let mut queues: Vec<SyncSender<SavedBlock>> = Vec::new();

        for _ in 0..cpus.into() {
            let (tx, rx) = mpsc::sync_channel(1024);
            queues.push(tx);

            let probl = &problematic;
            let ep = &epochs;
            s.spawn(move || {
                while let Ok(block) = rx.recv() {
                    if is_problematic(&block, ep) {
                        probl.lock().unwrap().push(block)
                    }
                }
            });
        }

        println!("Checking signatures...");
        for block in block_store.iter(&tx) {
            if checked % 100_000 == 0 {
                print!(
                    "\r{}% done - {} found",
                    (checked * 100) / total_blocks,
                    problematic.lock().unwrap().len()
                );
                std::io::stdout().flush().unwrap();
            }

            queues[(checked % queues.len() as u64) as usize]
                .send(block.clone())
                .unwrap();
            checked += 1;
        }
    });

    println!();
    println!("These blocks are problematic:");
    for block in problematic.lock().unwrap().iter() {
        println!("{} {}", block.hash(), block.account().encode_account());
    }
}

fn is_problematic(block: &SavedBlock, epochs: &Epochs) -> bool {
    let signer = get_signer(block, epochs);
    validate_message(&signer, block.hash().as_bytes(), block.signature()).is_err()
}

fn get_signer(block: &SavedBlock, epochs: &Epochs) -> PublicKey {
    if block.is_epoch() {
        epochs
            .epoch_signer(&block.link_field().unwrap())
            .unwrap()
            .into()
    } else {
        block.account().into()
    }
}

pub fn validate_message(
    public_key: &PublicKey,
    message: &[u8],
    signature: &Signature,
) -> Result<(), SignatureError> {
    let public = ed25519_dalek_blake2b::PublicKey::from_bytes(public_key.as_bytes())
        .map_err(|_| SignatureError::new())?;
    let sig = ed25519_dalek_blake2b::Signature::from_bytes(signature.as_bytes())
        .map_err(|_| SignatureError::new())?;
    public.verify_strict(message, &sig)
}
