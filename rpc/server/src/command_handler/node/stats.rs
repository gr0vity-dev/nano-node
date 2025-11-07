use std::time::SystemTime;

use rsnano_rpc_messages::{StatsArgs, StatsType, SuccessResponse};
use rsnano_utils::{
    container_info::ContainerInfo,
    stats::{StatsJsonWriter, StatsLogSink},
};

use crate::command_handler::RpcCommandHandler;

impl RpcCommandHandler {
    pub(crate) fn stats(&self, args: StatsArgs) -> anyhow::Result<serde_json::Value> {
        match args.stats_type {
            StatsType::Counters => {
                let mut sink = StatsJsonWriter::new();
                let now = SystemTime::now();
                sink.write_header("counters", now)?;
                {
                    let stats = self.node.stats();
                    for (key, value) in stats.iter() {
                        sink.write_counter_entry(
                            now,
                            key.stat,
                            key.detail,
                            key.dir.as_str(),
                            *value,
                        )?;
                    }
                }
                sink.finalize();
                sink.add(
                    "stat_duration_seconds",
                    self.ledger_services.stats.last_reset().as_secs(),
                );
                Ok(sink.finish())
            }
            StatsType::Samples => {
                let mut sink = StatsJsonWriter::new();
                self.ledger_services.stats.log_samples(&mut sink).unwrap();
                sink.add(
                    "stat_duration_seconds",
                    self.ledger_services.stats.last_reset().as_secs(),
                );
                Ok(sink.finish())
            }
            StatsType::Database => Ok(serde_json::to_value(
                self.ledger_services.ledger.memory_stats()?,
            )?),
            StatsType::Objects => Ok(ContainerInfo::builder()
                .node("node", self.node.container_info())
                .finish()
                .into_json()),
        }
    }

    pub(crate) fn stats_clear(&self) -> SuccessResponse {
        self.ledger_services.stats.clear();
        SuccessResponse::new()
    }
}
