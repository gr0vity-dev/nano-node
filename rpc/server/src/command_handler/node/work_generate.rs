use anyhow::bail;

use rsnano_rpc_messages::{WorkGenerateArgs, WorkGenerateDto};
use rsnano_types::{Block, BlockType, DifficultyV1, WorkRequest};

use super::difficulty_ledger;
use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn work_generate(&self, args: WorkGenerateArgs) -> anyhow::Result<WorkGenerateDto> {
        let default_difficulty = self.services.ledger.constants.work.threshold_base();

        let mut difficulty = args
            .difficulty
            .unwrap_or_else(|| default_difficulty.into())
            .inner();

        let max_difficulty = DifficultyV1::from_multiplier(
            self.node.config.max_work_generate_multiplier,
            default_difficulty,
        );

        // Validate difficulty
        if difficulty > max_difficulty
            || difficulty
                < self
                    .node
                    .network_params
                    .work
                    .threshold_entry(BlockType::State)
        {
            bail!("Difficulty out of range");
        }

        // Retrieving optional block
        if let Some(json_block) = args.block {
            let block: Block = json_block.into();
            if args.hash != block.root().into() {
                bail!("Block root mismatch");
            }
            // Recalculate difficulty if not provided
            if args.difficulty.is_none() && args.multiplier.is_none() {
                let any = self.services.ledger.any();
                difficulty = difficulty_ledger(self.node.clone(), &any, &block);
            }

            // If optional block difficulty is higher than requested difficulty, send error
            if self.node.network_params.work.difficulty_block(&block) >= difficulty {
                bail!("Provided work is already enough for given difficulty");
            }
        }

        if !self.services.work_factory.work_generation_enabled() {
            bail!("Work generation is disabled");
        }

        let work_request = WorkRequest::new(args.hash.into(), difficulty);
        let work = self
            .node
            .services()
            .work_factory
            .generate_work(work_request.clone());

        let Some(work) = work else {
            bail!("Work generation cancelled")
        };

        let result_difficulty = work_request.difficulty_of(work);
        let result_multiplier = DifficultyV1::to_multiplier(result_difficulty, default_difficulty);

        Ok(WorkGenerateDto {
            hash: args.hash,
            work,
            difficulty: result_difficulty.into(),
            multiplier: Some(result_multiplier.into()),
        })
    }
}
