use super::{DiscoveryState, RemoteService};
use super::{RemoteAddress, network};
use mdns_sd::{DaemonEvent, IfKind, RRType, ServiceDaemon, ServiceInfo};
use std::{
    collections::BTreeSet,
    future::pending,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

const SERVICE_TYPE: &str = "_noorwave._tcp.local.";
const INTERFACE_REFRESH: Duration = Duration::from_secs(5);
const SHUTDOWN_BUDGET: Duration = Duration::from_millis(900);
const UNREGISTER_BUDGET: Duration = Duration::from_millis(250);
const DAEMON_SHUTDOWN_BUDGET: Duration = Duration::from_millis(500);

fn build_registration(
    server_id: &str,
    hostname: &str,
    port: u16,
    addresses: impl IntoIterator<Item = Ipv4Addr>,
) -> mdns_sd::Result<ServiceInfo> {
    let instance_suffix: String = server_id.chars().take(8).collect();
    let instance = format!("NOORwave {instance_suffix}");
    let properties = [("path", "/remote"), ("protocol", "1"), ("id", server_id)];
    let addresses: Vec<_> = addresses.into_iter().map(IpAddr::V4).collect();
    ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        hostname,
        addresses.as_slice(),
        port,
        &properties[..],
    )
}

async fn announce_confirmed(remote: &RemoteService, hostname: &str, port: u16) {
    let display_hostname = hostname.trim_end_matches('.').to_string();
    remote
        .publish_discovery_state(
            DiscoveryState::Advertised,
            Some(display_hostname.clone()),
            Some(format!("http://{display_hostname}:{port}/remote")),
            None,
        )
        .await;
}

async fn hostname_changed(
    remote: &RemoteService,
    original: &str,
    new_hostname: &str,
    _port: u16,
) -> anyhow::Result<()> {
    if !remote.hostname().await.eq_ignore_ascii_case(original) {
        return Ok(());
    }
    remote.persist_effective_hostname(new_hostname).await?;
    remote
        .publish_discovery_state(
            DiscoveryState::Starting,
            Some(new_hostname.trim_end_matches('.').to_string()),
            None,
            None,
        )
        .await;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistrationPlan {
    hostname: String,
    addresses: Vec<Ipv4Addr>,
    interface_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkSnapshot {
    addresses: Vec<RemoteAddress>,
    registration: Option<RegistrationPlan>,
}

fn network_snapshot(
    listener: SocketAddr,
    hostname: String,
    interfaces: &[network::InterfaceSnapshot],
) -> NetworkSnapshot {
    let addresses = network::rank_addresses(listener, interfaces);
    let eligible: Vec<_> = interfaces
        .iter()
        .filter(|interface| {
            interface.active
                && interface.multicast
                && interface.kind == network::InterfaceKind::Physical
        })
        .cloned()
        .collect();
    let advertised = network::rank_addresses(listener, &eligible);
    let advertised_ips: Vec<_> = advertised
        .iter()
        .filter_map(|address| {
            url::Url::parse(&address.url)
                .ok()?
                .host_str()?
                .parse::<Ipv4Addr>()
                .ok()
        })
        .collect();
    let offered_ids: BTreeSet<_> = advertised
        .iter()
        .map(|address| address.id.clone())
        .collect();
    // `netdev` exposes Windows adapter IDs as stable GUIDs, while mdns-sd's
    // underlying Windows interface enumeration selects by FriendlyName (for
    // example, "Ethernet"). Using `id` here silently enabled no interface on
    // Windows, leaving discovery permanently in its starting state.
    let mut interface_names: Vec<_> = eligible
        .iter()
        .filter(|interface| {
            interface
                .ipv4
                .iter()
                .any(|ip| offered_ids.contains(&format!("{}:{ip}", interface.id)))
        })
        .map(|interface| interface.label.clone())
        .collect();
    interface_names.sort();
    interface_names.dedup();
    let registration =
        (!advertised_ips.is_empty() && !interface_names.is_empty()).then_some(RegistrationPlan {
            hostname,
            addresses: advertised_ips,
            interface_names,
        });
    NetworkSnapshot {
        addresses,
        registration,
    }
}

async fn capture_network(listener: SocketAddr, hostname: String) -> NetworkSnapshot {
    let interfaces = tokio::task::spawn_blocking(network::enumerate_interfaces)
        .await
        .unwrap_or_default();
    network_snapshot(listener, hostname, &interfaces)
}

struct ActiveResponder {
    daemon: ServiceDaemon,
    monitor: mdns_sd::Receiver<DaemonEvent>,
    fullname: String,
    plan: RegistrationPlan,
}

impl ActiveResponder {
    fn start(server_id: &str, port: u16, plan: RegistrationPlan) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let monitor = daemon.monitor()?;
        daemon.set_ip_check_interval(INTERFACE_REFRESH.as_secs() as u32)?;
        daemon.disable_interface(IfKind::All)?;
        for interface in &plan.interface_names {
            daemon.enable_interface(interface)?;
        }
        let service = build_registration(
            server_id,
            &plan.hostname,
            port,
            plan.addresses.iter().copied(),
        )?;
        let fullname = service.get_fullname().to_string();
        daemon.register(service)?;
        Ok(Self {
            daemon,
            monitor,
            fullname,
            plan,
        })
    }

    fn update(&mut self, server_id: &str, port: u16, plan: RegistrationPlan) -> anyhow::Result<()> {
        for interface in &self.plan.interface_names {
            self.daemon.disable_interface(interface)?;
        }
        for interface in &plan.interface_names {
            self.daemon.enable_interface(interface)?;
        }
        let service = build_registration(
            server_id,
            &plan.hostname,
            port,
            plan.addresses.iter().copied(),
        )?;
        self.fullname = service.get_fullname().to_string();
        self.daemon.register(service)?;
        self.plan = plan;
        Ok(())
    }

    async fn shutdown(self) {
        if let Ok(receiver) = self.daemon.unregister(&self.fullname) {
            let _ = tokio::time::timeout(UNREGISTER_BUDGET, receiver.recv_async()).await;
        }
        if let Ok(receiver) = self.daemon.shutdown() {
            let _ = tokio::time::timeout(DAEMON_SHUTDOWN_BUDGET, receiver.recv_async()).await;
        }
    }
}

pub struct DiscoveryHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl DiscoveryHandle {
    pub async fn start(
        remote: RemoteService,
        listener: SocketAddr,
        remote_assets_available: bool,
        process_shutdown: watch::Receiver<bool>,
    ) -> Option<Self> {
        if listener.ip().is_loopback() {
            remote
                .publish_discovery_state(DiscoveryState::Disabled, None, None, None)
                .await;
            return None;
        }
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(run_discovery(
            remote,
            listener,
            remote_assets_available,
            process_shutdown,
            shutdown_rx,
        ));
        Some(Self {
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(SHUTDOWN_BUDGET, &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn run_discovery(
    remote: RemoteService,
    listener: SocketAddr,
    remote_assets_available: bool,
    mut process_shutdown: watch::Receiver<bool>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let port = listener.port();
    let server_id = remote.server_id().to_string();
    let mut active: Option<ActiveResponder> = None;
    let mut ticker = tokio::time::interval(INTERFACE_REFRESH);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    if *process_shutdown.borrow() {
        remote.invalidate_ticket().await;
        remote
            .publish_discovery_state(DiscoveryState::Disabled, None, None, None)
            .await;
        return;
    }

    refresh(
        &remote,
        listener,
        remote_assets_available,
        &server_id,
        &mut active,
    )
    .await;

    loop {
        let monitor = active.as_ref().map(|responder| responder.monitor.clone());
        tokio::select! {
            _ = &mut shutdown => break,
            _ = process_shutdown.changed() => break,
            _ = ticker.tick() => {
                refresh(
                    &remote,
                    listener,
                    remote_assets_available,
                    &server_id,
                    &mut active,
                ).await;
            }
            event = next_event(monitor) => {
                match event {
                    Some(DaemonEvent::Announce(fullname, _))
                        if active.as_ref().is_some_and(|responder| responder.fullname.eq_ignore_ascii_case(&fullname)) =>
                    {
                        let hostname = remote.hostname().await;
                        announce_confirmed(&remote, &hostname, port).await;
                    }
                    Some(DaemonEvent::NameChange(change)) if change.rr_type == RRType::A => {
                        match hostname_changed(&remote, &change.original, &change.new_name, port).await {
                            Ok(()) => {
                                if let Some(responder) = active.as_mut() {
                                    let mut plan = responder.plan.clone();
                                    plan.hostname = remote.hostname().await;
                                    if let Err(error) = responder.update(&server_id, port, plan) {
                                        tracing::warn!(error = %error, "failed to update discovery after hostname collision");
                                        remote.publish_discovery_state(
                                            DiscoveryState::Unavailable,
                                            Some(remote.hostname().await.trim_end_matches('.').to_string()),
                                            None,
                                            Some("Local discovery is unavailable; use a direct IP address.".into()),
                                        ).await;
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "failed to persist conflict-resolved discovery hostname");
                                if let Some(responder) = active.take() {
                                    responder.shutdown().await;
                                }
                                remote.publish_discovery_state(
                                    DiscoveryState::Unavailable,
                                    None,
                                    None,
                                    Some("Local discovery could not save its conflict-free hostname; use a direct IP address.".into()),
                                ).await;
                            }
                        }
                    }
                    Some(DaemonEvent::IpAdd(_)) | Some(DaemonEvent::IpDel(_)) => {
                        refresh(
                            &remote,
                            listener,
                            remote_assets_available,
                            &server_id,
                            &mut active,
                        ).await;
                    }
                    Some(DaemonEvent::Error(error)) => {
                        tracing::warn!(error = %error, "mDNS responder reported an error");
                        remote.publish_discovery_state(
                            DiscoveryState::Unavailable,
                            Some(remote.hostname().await.trim_end_matches('.').to_string()),
                            None,
                            Some("Local discovery is unavailable; use a direct IP address.".into()),
                        ).await;
                        if let Some(responder) = active.take() {
                            responder.shutdown().await;
                        }
                    }
                    Some(_) => {}
                    None => {
                        if let Some(responder) = active.take() {
                            remote.publish_discovery_state(
                                DiscoveryState::Unavailable,
                                Some(remote.hostname().await.trim_end_matches('.').to_string()),
                                None,
                                Some("Local discovery stopped unexpectedly; use a direct IP address.".into()),
                            ).await;
                            responder.shutdown().await;
                        }
                    }
                }
            }
        }
    }

    if let Some(responder) = active.take() {
        responder.shutdown().await;
    }
    remote.replace_network_facts(Vec::new()).await;
    remote.invalidate_ticket().await;
    remote
        .publish_discovery_state(DiscoveryState::Disabled, None, None, None)
        .await;
}

async fn next_event(receiver: Option<mdns_sd::Receiver<DaemonEvent>>) -> Option<DaemonEvent> {
    match receiver {
        Some(receiver) => receiver.recv_async().await.ok(),
        None => pending().await,
    }
}

async fn refresh(
    remote: &RemoteService,
    listener: SocketAddr,
    remote_assets_available: bool,
    server_id: &str,
    active: &mut Option<ActiveResponder>,
) {
    let hostname = remote.hostname().await;
    let snapshot = capture_network(listener, hostname.clone()).await;
    remote
        .replace_network_facts(snapshot.addresses.clone())
        .await;

    let Some(plan) = snapshot.registration else {
        remote
            .publish_discovery_state(
                DiscoveryState::Unavailable,
                Some(hostname.trim_end_matches('.').to_string()),
                None,
                Some("No physical IPv4 LAN interface is available for local discovery.".into()),
            )
            .await;
        if let Some(responder) = active.take() {
            responder.shutdown().await;
        }
        return;
    };

    if !remote_assets_available {
        remote
            .publish_discovery_state(
                DiscoveryState::Unavailable,
                Some(hostname.trim_end_matches('.').to_string()),
                None,
                Some("The bundled phone remote assets are unavailable.".into()),
            )
            .await;
        if let Some(responder) = active.take() {
            responder.shutdown().await;
        }
        return;
    }

    match active {
        Some(responder) if responder.plan != plan => {
            if let Err(error) = responder.update(server_id, listener.port(), plan) {
                tracing::warn!(error = %error, "failed to refresh mDNS advertisement");
                remote
                    .publish_discovery_state(
                        DiscoveryState::Unavailable,
                        Some(hostname.trim_end_matches('.').to_string()),
                        None,
                        Some("Local discovery is unavailable; use a direct IP address.".into()),
                    )
                    .await;
            }
        }
        Some(_) => {}
        None => match ActiveResponder::start(server_id, listener.port(), plan) {
            Ok(responder) => {
                *active = Some(responder);
                remote
                    .publish_discovery_state(
                        DiscoveryState::Starting,
                        Some(hostname.trim_end_matches('.').to_string()),
                        None,
                        None,
                    )
                    .await;
            }
            Err(error) => {
                tracing::warn!(error = %error, "failed to start mDNS responder");
                remote
                    .publish_discovery_state(
                        DiscoveryState::Unavailable,
                        Some(hostname.trim_end_matches('.').to_string()),
                        None,
                        Some("Local discovery is unavailable; use a direct IP address.".into()),
                    )
                    .await;
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, schema};
    use axum::{Json, extract::State, http::StatusCode};

    fn service() -> RemoteService {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(schema::run_migrations).unwrap();
        RemoteService::new(db, "123456".into()).unwrap()
    }

    #[test]
    fn registration_uses_actual_port_ipv4_only_and_non_secret_txt_contract() {
        let addresses = [
            "192.168.1.24".parse::<Ipv4Addr>().unwrap(),
            "10.0.0.9".parse().unwrap(),
        ];
        let service = build_registration(
            "79d7b4b2-3333-4444-8888-123456789abc",
            "noorwave-2.local.",
            43123,
            addresses,
        )
        .unwrap();

        assert_eq!(service.get_type(), SERVICE_TYPE);
        assert_eq!(
            service.get_fullname(),
            "NOORwave 79d7b4b2._noorwave._tcp.local."
        );
        assert_eq!(service.get_hostname(), "noorwave-2.local.");
        assert_eq!(service.get_port(), 43123);
        assert_eq!(service.get_addresses_v4().len(), 2);
        assert_eq!(service.get_property_val_str("path"), Some("/remote"));
        assert_eq!(service.get_property_val_str("protocol"), Some("1"));
        assert_eq!(
            service.get_property_val_str("id"),
            Some("79d7b4b2-3333-4444-8888-123456789abc")
        );
        assert_eq!(service.get_properties().iter().count(), 3);
    }

    #[tokio::test]
    async fn hostname_collision_persists_clears_old_friendly_ticket_and_waits_for_announce() {
        let remote = service();
        remote
            .set_bound_listener(
                "0.0.0.0:43123".parse().unwrap(),
                super::super::HostControl::Desktop,
                true,
                true,
            )
            .await;
        remote
            .replace_network_facts(vec![super::super::RemoteAddress {
                id: "wifi:192.168.1.24".into(),
                label: "Wi-Fi".into(),
                url: "http://192.168.1.24:43123/remote".into(),
                kind: "lan".into(),
                recommended: true,
            }])
            .await;
        announce_confirmed(&remote, "noorwave.local.", 43123).await;
        let response = super::super::create_ticket_handler(
            State(remote.clone()),
            Ok(Json(super::super::CreatePairingRequest {
                address_id: Some("friendly".into()),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        hostname_changed(&remote, "noorwave.local.", "noorwave-2.local.", 43123)
            .await
            .unwrap();
        assert_eq!(remote.hostname().await, "noorwave-2.local.");
        assert!(remote.0.ticket.lock().await.is_none());
        let runtime = remote.0.runtime.read().await;
        assert_eq!(
            runtime.discovery_state,
            super::super::DiscoveryState::Starting
        );
        assert_eq!(
            runtime.discovery_hostname.as_deref(),
            Some("noorwave-2.local")
        );
        assert!(runtime.friendly_url.is_none());
        drop(runtime);

        announce_confirmed(&remote, "noorwave-2.local.", 43123).await;
        let runtime = remote.0.runtime.read().await;
        assert_eq!(
            runtime.discovery_state,
            super::super::DiscoveryState::Advertised
        );
        assert_eq!(
            runtime.friendly_url.as_deref(),
            Some("http://noorwave-2.local:43123/remote")
        );
    }

    #[test]
    fn direct_addresses_include_explicit_fallbacks_but_mdns_uses_physical_multicast_lan_only() {
        let interfaces = vec![
            network::InterfaceSnapshot {
                id: "{windows-adapter-guid}".into(),
                label: "Wi-Fi".into(),
                kind: network::InterfaceKind::Physical,
                active: true,
                multicast: true,
                ipv4: vec!["192.168.1.24".parse().unwrap()],
            },
            network::InterfaceSnapshot {
                id: "ethernet".into(),
                label: "Ethernet".into(),
                kind: network::InterfaceKind::Physical,
                active: true,
                multicast: false,
                ipv4: vec!["10.0.0.9".parse().unwrap()],
            },
            network::InterfaceSnapshot {
                id: "vpn".into(),
                label: "VPN".into(),
                kind: network::InterfaceKind::Other,
                active: true,
                multicast: true,
                ipv4: vec!["10.8.0.2".parse().unwrap()],
            },
        ];

        let snapshot = network_snapshot(
            "0.0.0.0:43123".parse().unwrap(),
            "noorwave.local.".into(),
            &interfaces,
        );
        assert_eq!(snapshot.addresses.len(), 3);
        let registration = snapshot.registration.unwrap();
        assert_eq!(
            registration.addresses,
            vec!["192.168.1.24".parse::<Ipv4Addr>().unwrap()]
        );
        assert_eq!(registration.interface_names, vec!["Wi-Fi"]);
    }

    #[tokio::test]
    async fn stuck_discovery_task_is_forced_down_inside_the_shutdown_budget() {
        let (shutdown, _shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async { pending::<()>().await });
        let handle = DiscoveryHandle {
            shutdown: Some(shutdown),
            task: Some(task),
        };

        tokio::time::timeout(Duration::from_secs(1), handle.shutdown())
            .await
            .expect("discovery handle exceeded its one-second shutdown budget");
    }

    #[tokio::test]
    async fn retained_shutdown_state_prevents_a_late_responder_start() {
        let remote = service();
        remote
            .set_bound_listener(
                "0.0.0.0:43123".parse().unwrap(),
                super::super::HostControl::Desktop,
                true,
                true,
            )
            .await;
        let (process_shutdown, process_shutdown_rx) = watch::channel(false);
        process_shutdown.send(true).unwrap();
        let (_manual_shutdown, manual_shutdown_rx) = oneshot::channel();

        tokio::time::timeout(
            Duration::from_millis(100),
            run_discovery(
                remote.clone(),
                "0.0.0.0:43123".parse().unwrap(),
                true,
                process_shutdown_rx,
                manual_shutdown_rx,
            ),
        )
        .await
        .expect("retained process shutdown should be observed before interface work");

        let runtime = remote.0.runtime.read().await;
        assert_eq!(runtime.discovery_state, DiscoveryState::Disabled);
        assert!(runtime.friendly_url.is_none());
    }
}
