use crate::command_handler::RpcCommandHandler;
use rsnano_node::subsystems::consensus::OnlineRepsSnapshot;
use rsnano_rpc_messages::{ConfirmationQuorumArgs, ConfirmationQuorumResponse, PeerDetailsDto};

impl RpcCommandHandler {
    pub(crate) fn confirmation_quorum(
        &self,
        args: ConfirmationQuorumArgs,
    ) -> ConfirmationQuorumResponse {
        let snapshot = self.consensus.online_reps_snapshot();
        create_response(args, &snapshot)
    }
}

fn create_response(
    args: ConfirmationQuorumArgs,
    online_reps: &OnlineRepsSnapshot,
) -> ConfirmationQuorumResponse {
    let mut result = ConfirmationQuorumResponse {
        quorum_delta: online_reps.quorum_delta,
        online_weight_quorum_percent: online_reps.quorum_percent.into(),
        online_weight_minimum: online_reps.online_weight_minimum,
        online_stake_total: online_reps.online_weight,
        trended_stake_total: online_reps.trended_weight,
        peers_stake_total: online_reps.peered_weight,
        peers: None,
    };

    if args.include_peer_details() {
        let peers = online_reps
            .peered_reps
            .iter()
            .map(|rep| PeerDetailsDto {
                account: rep.rep_key.into(),
                ip: rep.channel.peer_addr(),
                weight: rep.weight,
            })
            .collect();

        result.peers = Some(peers);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::create_response;
    use crate::command_handler::test_rpc_command;
    use rsnano_node::{representatives::OnlineReps, subsystems::consensus::OnlineRepsSnapshot};
    use rsnano_rpc_messages::{ConfirmationQuorumArgs, ConfirmationQuorumResponse, RpcCommand};
    use rsnano_types::Amount;

    #[test]
    fn confirmation_quorum_command() {
        let result: ConfirmationQuorumResponse =
            test_rpc_command(RpcCommand::confirmation_quorum());
        assert!(result.quorum_delta > Amount::ZERO);
    }

    #[test]
    fn quorum_response() {
        let online_reps = OnlineReps::new_test_instance();
        let snapshot = OnlineRepsSnapshot {
            quorum_delta: online_reps.quorum_delta(),
            quorum_percent: online_reps.quorum_percent(),
            online_weight_minimum: online_reps.online_weight_minimum(),
            online_weight: online_reps.online_weight(),
            trended_weight: online_reps.trended_or_minimum_weight(),
            peered_weight: online_reps.peered_weight(),
            minimum_principal_weight: online_reps.minimum_principal_weight(),
            peered_reps: online_reps.peered_reps(),
            online_reps: online_reps.online_reps().collect(),
        };
        let response = create_response(ConfirmationQuorumArgs { peer_details: None }, &snapshot);
        assert_eq!(response.quorum_delta, snapshot.quorum_delta);
        assert_eq!(
            response.online_weight_quorum_percent,
            snapshot.quorum_percent.into()
        );
        assert_eq!(
            response.online_weight_minimum,
            snapshot.online_weight_minimum
        );
        assert_eq!(response.online_stake_total, snapshot.online_weight);
        assert_eq!(response.trended_stake_total, snapshot.trended_weight);
        assert_eq!(response.peers_stake_total, snapshot.peered_weight);
        assert!(response.peers.is_none());
    }

    #[test]
    fn quorum_response_with_peers() {
        let online_reps = OnlineReps::new_test_instance();
        let snapshot = OnlineRepsSnapshot {
            quorum_delta: online_reps.quorum_delta(),
            quorum_percent: online_reps.quorum_percent(),
            online_weight_minimum: online_reps.online_weight_minimum(),
            online_weight: online_reps.online_weight(),
            trended_weight: online_reps.trended_or_minimum_weight(),
            peered_weight: online_reps.peered_weight(),
            minimum_principal_weight: online_reps.minimum_principal_weight(),
            peered_reps: online_reps.peered_reps(),
            online_reps: online_reps.online_reps().collect(),
        };
        let response = create_response(
            ConfirmationQuorumArgs {
                peer_details: Some(true.into()),
            },
            &snapshot,
        );
        assert_eq!(response.quorum_delta, snapshot.quorum_delta);
        let peers = response.peers.unwrap();
        assert_eq!(peers.len(), snapshot.peered_reps.len());
    }
}
