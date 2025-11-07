use super::HashArgs;
use crate::cli::{GlobalArgs, build_node};
use anyhow::anyhow;
use rsnano_types::BlockHash;

pub(crate) fn roll_back(global_args: GlobalArgs, args: HashArgs) -> anyhow::Result<()> {
    let node = build_node(&global_args)?;
    let block_hash =
        BlockHash::decode_hex(&args.hash).ok_or_else(|| anyhow!("Invalid block hash"))?;
    println!("Rolling back {block_hash:?}");
    let rolled_back = node.services().ledger.roll_back(&block_hash)?;
    println!("Block rollback complete");
    println!("Rolled back {rolled_back} dependent blocks");
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        CommandLineArgs,
        cli::{
            Commands,
            commands::ledger::{HashArgs, LedgerCommand, LedgerSubcommands},
        },
    };
    use clap::Parser;

    #[test]
    fn parse_roll_back_command() {
        let cmd =
            CommandLineArgs::try_parse_from(["nulled_node_bin", "ledger", "roll-back", "--hash=1"])
                .unwrap();
        assert_eq!(
            cmd,
            CommandLineArgs {
                command: Some(Commands::Ledger(LedgerCommand {
                    subcommand: Some(LedgerSubcommands::RollBack(HashArgs {
                        hash: "1".to_string()
                    })),
                })),
                ..Default::default()
            }
        )
    }
}
