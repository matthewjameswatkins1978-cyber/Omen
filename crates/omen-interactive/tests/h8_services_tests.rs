use omen_core::ResourceUri;
use omen_interactive::InteractiveSession;
use omen_interactive::services::{ManagedService, ServiceRegistry, ServiceState};
use tempfile::tempdir;

#[test]
fn test_service_registry_lifecycle() {
    let registry = ServiceRegistry::new();
    let uri = ResourceUri::parse("proc://backend").unwrap();

    let service = ManagedService {
        name: "backend".into(),
        resource_uri: uri.clone(),
        pid: Some(12345),
        command: "cargo run --bin backend".into(),
        state: ServiceState::Running,
        uptime_secs: 42,
    };

    registry.register(service);

    let list = registry.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "backend");
    assert_eq!(list[0].state, ServiceState::Running);

    let svc = registry.get("backend").unwrap();
    assert_eq!(svc.pid, Some(12345));

    // Stop service
    let stopped = registry.stop("backend").unwrap();
    assert!(stopped);

    let svc_after = registry.get("backend").unwrap();
    assert_eq!(svc_after.state, ServiceState::Stopped);
}

#[test]
fn test_semantic_actions_services_and_stop() {
    let temp = tempdir().unwrap();
    let mut session = InteractiveSession::new(temp.path().to_path_buf(), None).unwrap();

    // Register a global service
    let uri = ResourceUri::parse("proc://web").unwrap();
    let service = ManagedService {
        name: "web".into(),
        resource_uri: uri,
        pid: Some(9999),
        command: "npm run dev".into(),
        state: ServiceState::Running,
        uptime_secs: 120,
    };
    ServiceRegistry::global().register(service);

    // 1. :services
    let exit_services = session.dispatch_input(":services").unwrap();
    assert!(exit_services.is_zero());

    // 2. :status @service.web
    let exit_status = session.dispatch_input(":status @service.web").unwrap();
    assert!(exit_status.is_zero());

    // 3. :stop @service.web
    let exit_stop = session.dispatch_input(":stop @service.web").unwrap();
    assert!(exit_stop.is_zero());

    // 4. Check state in global registry
    let web_svc = ServiceRegistry::global().get("web").unwrap();
    assert_eq!(web_svc.state, ServiceState::Stopped);
}
