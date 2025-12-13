use std::fs;

/// Integration test for rebuild + cancel scenarios
/// Tests that tree display shows correct statuses after cancellation
use nix_btm::derivation_tree::DrvNode;
use nix_btm::{
    handle_internal_json::{
        Drv, JobId, JobStatus, JobsStateInner, RequesterId, TargetStatus,
    },
    tree_generation::{PruneType, TreeCache, gen_drv_tree_leaves_from_state},
};

/// Simulate parsing a log file by extracting target reference and root drv
/// Returns (target_reference, root_drv)
fn extract_target_info(log_content: &str) -> Option<(String, Drv)> {
    for line in log_content.lines() {
        if line.contains("evaluating derivation") {
            // Extract target reference like
            // "github:nixos/nix/2.19.1#packages.aarch64-darwin.default"
            if let Some(start_idx) = line.find("evaluating derivation '") {
                let after = &line[start_idx + 23..]; // len("evaluating derivation '")
                if let Some(end_idx) = after.find('\'') {
                    let target_ref = &after[..end_idx];

                    // Find the root drv in subsequent lines
                    for drv_line in log_content.lines() {
                        if drv_line.contains("checking outputs") {
                            // Extract drv like
                            // "/nix/store/
                            // 70w0z89sspbw633dscf2idk6r95ziimf-nix-2.19.1.drv"
                            if let Some(drv_start) =
                                drv_line.find("/nix/store/")
                            {
                                let drv_str = &drv_line[drv_start..];
                                if let Some(drv_end) = drv_str.find(".drv") {
                                    let full_drv = &drv_str[..=drv_end + 3];
                                    if let Some(drv) = parse_drv_path(full_drv)
                                    {
                                        return Some((
                                            target_ref.to_string(),
                                            drv,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Parse drv path like
/// "/nix/store/70w0z89sspbw633dscf2idk6r95ziimf-nix-2.19.1.drv" Returns Drv
/// with name="nix-2.19.1" and hash="70w0z89sspbw633dscf2idk6r95ziimf"
fn parse_drv_path(path: &str) -> Option<Drv> {
    // Path format: /nix/store/{hash}-{name}.drv
    let parts: Vec<&str> = path.split('/').collect();
    let filename = parts.last()?;
    let without_drv = filename.strip_suffix(".drv")?;
    let mut split = without_drv.splitn(2, '-');
    let hash = split.next()?.to_string();
    let name = split.next()?.to_string();

    Some(Drv { name, hash })
}
