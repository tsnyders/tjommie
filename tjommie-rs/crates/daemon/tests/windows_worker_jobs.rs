//! T2.3 port of `windows-worker-jobs.test.ts`: job descriptors are checked in memory, without creating objects.
//! Not ported: the receipt checks (`Add-TrustedWorkerJobs`). They guarded the JSON receipts the TypeScript daemon
//! handed its PowerShell wrapper; the Rust daemon holds its job handles itself, so no receipt crosses a process.
#![cfg(windows)]

use daemon::win::logon::Descriptor;
use daemon::win::worker::assert_job_descriptor;

const OWNER: &str = "S-1-5-21-1-2-3-1000";
const PROVIDER: &str = "S-1-5-21-1-2-3-1001";
const REVIEW: &str = "S-1-5-21-1-2-3-1002";

#[test]
fn accepts_only_the_owner_system_administrators_job_descriptor() {
    let safe = format!("O:{OWNER}D:P(A;;GA;;;{OWNER})(A;;GA;;;SY)(A;;GA;;;BA)");
    assert_job_descriptor(&Descriptor::from_sddl(&safe).unwrap(), OWNER).unwrap();
    for unsafe_sddl in [
        format!("{safe}(A;;GR;;;{PROVIDER})"),
        format!("{safe}(A;;GA;;;{REVIEW})"),
        format!("O:{PROVIDER}D:P(A;;GA;;;{OWNER})"),
        format!("O:{OWNER}D:NO_ACCESS_CONTROL"),
        format!("O:{OWNER}D:P(A;OICI;GA;;;{OWNER})"),
        format!("O:{OWNER}D:P(D;;GA;;;{OWNER})"),
    ] {
        assert!(
            assert_job_descriptor(&Descriptor::from_sddl(&unsafe_sddl).unwrap(), OWNER).is_err(),
            "{unsafe_sddl}"
        );
    }
}
