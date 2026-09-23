//! Windows Service-specific generation logic.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

const SERVICE_MODULE: &str = r#"use std::{
    ffi::OsString,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

const SERVICE_NAME: &str = "RustPackerService";

define_windows_service!(ffi_service_main, service_main);

pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(_error) = run() {
        std::process::exit(1);
    }
}

fn run() -> windows_service::Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel();
    let status = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    report_status(&status, ServiceState::StartPending)?;
    report_status(&status, ServiceState::Running)?;

    super::run_payload();

    loop {
        match stop_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    report_status(&status, ServiceState::StopPending)?;
    report_status(&status, ServiceState::Stopped)
}

fn report_status(handle: &ServiceStatusHandle, state: ServiceState) -> windows_service::Result<()> {
    let pending = matches!(
        state,
        ServiceState::StartPending | ServiceState::StopPending
    );
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: if pending { 1 } else { 0 },
        wait_hint: if pending {
            Duration::from_secs(10)
        } else {
            Duration::ZERO
        },
        process_id: None,
    })
}
"#;

pub(super) fn apply_service_format(main_rs_path: &Path) -> Result<()> {
    let src_dir = main_rs_path.parent().context("main.rs has no parent")?;
    fs::write(src_dir.join("service.rs"), SERVICE_MODULE)
        .context("Failed to write service.rs")?;
    Ok(())
}

pub(super) fn wrap_main_for_service(main_rs_path: &Path) -> Result<()> {
    let content = fs::read_to_string(main_rs_path)
        .context("Failed to read main.rs for service wrapping")?;
    
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    
    let mut inner_attrs = Vec::new();
    let mut rest = Vec::new();
    let mut in_attrs = true;
    
    for line in lines {
        if in_attrs && line.trim().starts_with("#![") {
            inner_attrs.push(line);
        } else {
            in_attrs = false;
            rest.push(line);
        }
    }
    
    let rest_code = rest.join("\n").replace("fn main()", "fn real_main()");
    
    let service_wrapper = r#"#[cfg(windows)]
mod service;

#[cfg(windows)]
fn run_payload() {
    real_main();
}

#[cfg(windows)]
fn main() {
    if let Err(_) = service::dispatch() {
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This binary must be run as a Windows service.");
    std::process::exit(1);
}
"#;
    
    let wrapped = format!(
        "{}\n\n{}\n\n{}",
        inner_attrs.join("\n"),
        rest_code,
        service_wrapper
    );
    
    fs::write(main_rs_path, wrapped)
        .context("Failed to write wrapped main.rs for service format")?;
    
    Ok(())
}

pub(super) fn add_service_dependency(cargo_toml_path: &Path) -> Result<()> {
    let content = fs::read_to_string(cargo_toml_path)
        .context("Failed to read Cargo.toml")?;
    
    let target_section = r#"
[target.'cfg(windows)'.dependencies]
windows-service = "0.8.0"
"#;
    
    let updated = if content.contains("[target.'cfg(windows)'.dependencies]") {
        content
    } else {
        format!("{}{}", content, target_section)
    };
    
    fs::write(cargo_toml_path, updated)
        .context("Failed to update Cargo.toml with service dependency")?;
    
    Ok(())
}
