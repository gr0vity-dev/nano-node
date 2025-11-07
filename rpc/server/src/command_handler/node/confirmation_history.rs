use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::{
    ConfirmationEntry, ConfirmationHistoryArgs, ConfirmationHistoryResponse, ConfirmationStats,
};
use std::time::{Duration, UNIX_EPOCH};

impl RpcCommandHandler {
    pub(crate) fn confirmation_history(
        &self,
        args: ConfirmationHistoryArgs,
    ) -> ConfirmationHistoryResponse {
        let mut elections = Vec::new();
        let mut running_total = Duration::ZERO;
        let hash = args.hash.unwrap_or_default();
        for election in self.node.services.recently_cemented.lock().unwrap().iter() {
            if hash.is_zero() || election.winner.hash() == hash {
                elections.push(ConfirmationEntry {
                    hash: election.winner.hash(),
                    duration: (election.election_duration.as_millis() as u64).into(),
                    time: (election
                        .election_end
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64)
                        .into(),
                    tally: election.tally,
                    final_tally: election.final_tally,
                    blocks: election.block_count.into(),
                    voters: election.voter_count.into(),
                    request_count: 0.into(), // currently not supported in RsNano
                });
            }
            running_total += election.election_duration;
        }

        ConfirmationHistoryResponse {
            confirmation_stats: ConfirmationStats {
                count: (elections.len() as u32).into(),
                average: if elections.is_empty() {
                    None
                } else {
                    Some((running_total.as_millis() as u64 / elections.len() as u64).into())
                },
            },
            confirmations: elections,
        }
    }
}
