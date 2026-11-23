//! Windows 防火墙自动放行/移除模块 (`netsh advfirewall`)。
//!
//! 职责：在安装/卸载 LanVault Windows 服务时，自动管理局域网 TCP 数据/Web 端口、
//! mDNS UDP 5353 与局域网发现广播兜底 UDP 7843 端口的入站放行规则（放行 Private 与 Domain 网络配置文件）。

use anyhow::{bail, Result};
use lanvault_core::UDP_FALLBACK_PORT;

/// 防火墙规则名称常量。
pub const RULE_NAME_TCP: &str = "EveryoneNAS LAN TCP";
pub const RULE_NAME_MDNS_UDP: &str = "EveryoneNAS mDNS UDP 5353";
pub const RULE_NAME_DISCOVERY_UDP: &str = "EveryoneNAS Discovery UDP 7843";
pub const MDNS_UDP_PORT: u16 = 5353;
pub const DISCOVERY_FALLBACK_UDP_PORT: u16 = UDP_FALLBACK_PORT;

/// 自动放行 LanVault 局域网 TCP 端口、mDNS UDP 5353 与发现广播兜底 UDP 7843 端口。
#[cfg(windows)]
pub fn ensure_firewall_rules(tcp_port: u16) -> Result<()> {
    use tracing::info;

    if tcp_port == 0 {
        bail!("tcp_port must be non-zero");
    }

    // 先清理可能存在的旧端口同名规则，确保重复安装幂等。
    delete_rule_if_present(RULE_NAME_TCP)?;
    delete_rule_if_present(RULE_NAME_MDNS_UDP)?;
    delete_rule_if_present(RULE_NAME_DISCOVERY_UDP)?;

    add_inbound_rule(RULE_NAME_TCP, "TCP", tcp_port)?;
    add_inbound_rule(RULE_NAME_MDNS_UDP, "UDP", MDNS_UDP_PORT)?;
    add_inbound_rule(RULE_NAME_DISCOVERY_UDP, "UDP", DISCOVERY_FALLBACK_UDP_PORT)?;

    info!(
        tcp_port,
        mdns_port = MDNS_UDP_PORT,
        discovery_udp_port = DISCOVERY_FALLBACK_UDP_PORT,
        "configured Windows Firewall allow rules for LanVault"
    );
    Ok(())
}

/// 移除 LanVault 创建的全部 Windows 防火墙规则。
#[cfg(windows)]
pub fn remove_firewall_rules() -> Result<()> {
    use tracing::info;

    delete_rule_if_present(RULE_NAME_TCP)?;
    delete_rule_if_present(RULE_NAME_MDNS_UDP)?;
    delete_rule_if_present(RULE_NAME_DISCOVERY_UDP)?;
    info!("removed Windows Firewall rules for LanVault");
    Ok(())
}

#[cfg(windows)]
fn add_inbound_rule(name: &str, protocol: &str, port: u16) -> Result<()> {
    use std::process::Command;

    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={name}"),
            "dir=in",
            "action=allow",
            &format!("protocol={protocol}"),
            &format!("localport={port}"),
            "profile=private,domain",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("netsh add rule `{name}` failed: {stdout} {stderr}");
    }
    Ok(())
}

#[cfg(windows)]
fn rule_exists(name: &str) -> Result<bool> {
    use std::process::Command;

    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "show",
            "rule",
            &format!("name={name}"),
        ])
        .output()?;

    Ok(output.status.success())
}

#[cfg(windows)]
fn delete_rule_if_present(name: &str) -> Result<()> {
    use std::process::Command;

    if !rule_exists(name)? {
        return Ok(());
    }

    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={name}"),
        ])
        .output()?;

    if !output.status.success() {
        let combined = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        bail!("netsh delete rule `{name}` failed: {combined}");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn ensure_firewall_rules(_tcp_port: u16) -> Result<()> {
    bail!("Windows Firewall management (`netsh advfirewall`) is only supported on Windows")
}

#[cfg(not(windows))]
pub fn remove_firewall_rules() -> Result<()> {
    bail!("Windows Firewall management (`netsh advfirewall`) is only supported on Windows")
}
