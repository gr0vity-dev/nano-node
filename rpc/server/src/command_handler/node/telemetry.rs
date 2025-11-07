use crate::command_handler::RpcCommandHandler;
use anyhow::{anyhow, bail};
use rsnano_rpc_messages::{RawTelemetryResponse, TelemetryArgs, TelemetryDto, TelemetryResponse};
use std::net::SocketAddrV6;

impl RpcCommandHandler {
    pub(crate) fn telemetry(&self, args: TelemetryArgs) -> anyhow::Result<TelemetryResponse> {
        if args.address.is_some() || args.port.is_some() {
            let endpoint = Self::get_endpoint(&args)?;

            if self.is_local_address(&endpoint) {
                // Requesting telemetry metrics locally
                let data = self.telemetry_services.telemetry.local_telemetry();
                Ok(TelemetryResponse::Single(data.into()))
            } else {
                let telemetry = self
                    .telemetry_services
                    .telemetry
                    .get_telemetry(&endpoint)
                    .ok_or_else(|| anyhow!("Peer not found"))?;

                Ok(TelemetryResponse::Single(telemetry.into()))
            }
        } else {
            // By default, local telemetry metrics are returned,
            // setting "raw" to true returns metrics from all nodes requested.
            let output_raw = args.raw.unwrap_or_default().inner();
            if output_raw {
                let all_telemetries = self.telemetry_services.telemetry.get_all_telemetries();
                let mut responses = Vec::new();
                for (addr, data) in all_telemetries {
                    let mut metric = TelemetryDto::from(data);
                    metric.address = Some(*addr.ip());
                    metric.port = Some(addr.port().into());
                    responses.push(metric);
                }
                Ok(TelemetryResponse::Raw(RawTelemetryResponse {
                    metrics: responses,
                }))
            } else {
                // Default case without any parameters, requesting telemetry metrics locally
                let data = self.telemetry_services.telemetry.local_telemetry();
                Ok(TelemetryResponse::Single(data.into()))
            }
        }
    }

    fn get_endpoint(args: &TelemetryArgs) -> anyhow::Result<SocketAddrV6> {
        let Some(address) = args.address else {
            bail!("Both port and address required");
        };
        let Some(port) = args.port else {
            bail!("Both port and address required");
        };

        Ok(SocketAddrV6::new(address, port.into(), 0, 0))
    }

    fn is_local_address(&self, addr: &SocketAddrV6) -> bool {
        addr.ip().is_loopback()
            && addr.port() == self.telemetry_services.tcp_listener.local_address().port()
    }
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv6Addr, sync::Arc};

    use rsnano_messages::TelemetryData;
    use rsnano_node::Node;
    use rsnano_rpc_messages::{RpcCommand, RpcError, TelemetryArgs, TelemetryDto};

    use crate::command_handler::{test_rpc_command, test_rpc_command_with_node};

    #[test]
    fn fails_when_only_port_provided() {
        let cmd = RpcCommand::Telemetry(TelemetryArgs {
            raw: None,
            address: None,
            port: Some(123.into()),
        });
        let error: RpcError = test_rpc_command(cmd);
        assert_eq!(error.error, "Both port and address required")
    }

    #[test]
    fn fails_when_only_address_provided() {
        let cmd = RpcCommand::Telemetry(TelemetryArgs {
            raw: None,
            address: Some(Ipv6Addr::LOCALHOST),
            port: None,
        });
        let error: RpcError = test_rpc_command(cmd);
        assert_eq!(error.error, "Both port and address required")
    }

    #[test]
    fn returns_local_telemetry_by_default() {
        let cmd = RpcCommand::Telemetry(TelemetryArgs {
            raw: None,
            address: None,
            port: None,
        });

        let node = Arc::new(Node::new_null());
        let expected = node.services().telemetry.local_telemetry();
        let result: TelemetryDto = test_rpc_command_with_node(cmd, node);
        assert_result(expected, result);
    }

    #[test]
    fn returns_local_telemetry_if_local_address_requested() {
        let node = Arc::new(Node::new_null());
        let cmd = RpcCommand::Telemetry(TelemetryArgs {
            raw: None,
            address: Some(Ipv6Addr::LOCALHOST),
            port: Some(node.services().tcp_listener.local_address().port().into()),
        });

        let expected = node.services().telemetry.local_telemetry();
        let result: TelemetryDto = test_rpc_command_with_node(cmd, node);
        assert_result(expected, result);
    }

    #[test]
    fn fails_when_peer_not_found() {
        let node = Arc::new(Node::new_null());
        let cmd = RpcCommand::Telemetry(TelemetryArgs {
            raw: None,
            address: Some(Ipv6Addr::LOCALHOST),
            port: Some(12345.into()),
        });

        let error: RpcError = test_rpc_command_with_node(cmd, node);
        assert_eq!(error.error, "Peer not found");
    }

    fn assert_result(expected: TelemetryData, mut result: TelemetryDto) {
        let mut expected_dto: TelemetryDto = expected.into();
        expected_dto.signature = None;
        result.signature = None;
        assert_eq!(result, expected_dto);
    }
}
