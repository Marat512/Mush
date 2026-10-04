//! Process-wide tokio runtime (MUSH-A'-ANDROID-SPEC §5.4).
//!
//! One multi-thread runtime (2 workers) shared by all nodes in the process —
//! the JNI contract is handle-based, and several nodes may coexist (tests run
//! two in one process). JNI entry points never block on it for real work;
//! heavy operations are `spawn`ed and report back through the event sink.

use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();

pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("mush-jni")
            .build()
            .expect("tokio runtime")
    })
}
