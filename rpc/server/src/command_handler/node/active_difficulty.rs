use crate::command_handler::RpcCommandHandler;
use rsnano_rpc_messages::ActiveDifficultyResponse;

impl RpcCommandHandler {
    pub(crate) fn active_difficulty(&self) -> ActiveDifficultyResponse {
        let work = &self.node.network_params().work;

        ActiveDifficultyResponse {
            deprecated: "1".to_owned(),
            network_minimum: work.threshold_base().into(),
            network_receive_minimum: work.epoch_2_receive.into(),
            network_current: work.threshold_base().into(),
            network_receive_current: work.epoch_2_receive.into(),
            multiplier: 1.0.into(),
            difficulty_trend: Some(1.0.into()),
        }
    }
}
