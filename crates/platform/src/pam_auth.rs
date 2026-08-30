//! PAM authentication for srdwm's own session-lock UI
//! (`crates/wayland/src/native_lock.rs`) - the one piece of that feature
//! that must never be "close enough": this is what stands between a typed
//! password and actually unlocking the session.
//!
//! Uses `pam_client`'s application-side API (the same shape swaylock,
//! gtklock, and every other real screen locker use) rather than reading
//! `/etc/shadow` by hand - PAM already handles the privilege boundary
//! correctly (on a normal distro, `pam_unix.so` shells out to the setuid
//! `unix_chkpwd` helper to compare the password, so this process never
//! needs elevated privileges or shadow-file access itself) and honors
//! whatever the system's actual auth policy is (a YubiKey module, an
//! account lockout policy, anything `/etc/pam.d/srdwm` enables), not just
//! a plain password check.
//!
//! Needs `/etc/pam.d/srdwm` to exist (any distro's default `login`-derived
//! policy works, e.g. `auth include login`) - a service with no PAM
//! config file at all fails every authentication attempt, not falls back
//! to some default. That file is a root-owned system config change,
//! deliberately not written by this code.

use pam_client::conv_mock::Conversation;
use pam_client::{Context, Flag};

/// The PAM service name - see `Context::new`'s own docs: this is what
/// selects the policy from `/etc/pam.d/<service>`.
const SERVICE: &str = "srdwm";

/// Verifies `password` for `username` against the system's real PAM
/// policy. `true` only for a genuine, complete authentication success
/// (both `authenticate` *and* `acct_mgmt`, so a correct password on a
/// locked or expired account still correctly fails) - every other
/// outcome, including a PAM setup problem that has nothing to do with the
/// password itself, resolves to `false`. Deliberately no distinction
/// between "wrong password" and "something is broken" in the return value
/// - fail secure means every non-success path stays locked, not just the
///   ones that are the user's own fault. Logged at `warn` for whoever's
///   debugging a setup problem, never at a level that would put the
///   password itself in a log.
pub fn authenticate(username: &str, password: &str) -> bool {
    let conversation = Conversation::with_credentials(username, password);
    let mut context = match Context::new(SERVICE, Some(username), conversation) {
        Ok(ctx) => ctx,
        Err(e) => {
            log::warn!("session lock: failed to start PAM context for service '{SERVICE}': {e} ({:?})", e.code());
            return false;
        }
    };
    if let Err(e) = context.authenticate(Flag::NONE) {
        log::warn!("session lock: PAM authentication failed: {e} ({:?})", e.code());
        return false;
    }
    if let Err(e) = context.acct_mgmt(Flag::NONE) {
        log::warn!("session lock: PAM account check failed: {e} ({:?})", e.code());
        return false;
    }
    true
}
