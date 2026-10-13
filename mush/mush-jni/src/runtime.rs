//! Process-wide tokio runtime (MUSH-A'-ANDROID-SPEC §5.4).
//!
//! One multi-thread runtime (2 workers) shared by all nodes in the process —
//! the JNI contract is handle-based, and several nodes may coexist (tests run
//! two in one process). JNI entry points never block on it for real work;
//! heavy operations are `spawn`ed and report back through the event sink.

use std::future::Future;

use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();

/// Panic'и Rust-потоков на Android уходят в stderr, который в logcat не
/// попадает — воркер runtime может умереть бесшумно (MUSH-HARDWARE-TEST-PLAN
/// §3.5 п.15: на Redmi 5 воркеров 1 вместо 2, panic'ов в logcat нет).
/// Хук дублирует payload + location через tracing (тег `mush` в logcat),
/// затем отдаёт сообщение прежнему хуку (на host — печать в stderr, там
/// stderr живой). catch_unwind вокруг tracing: падение внутри самого хука
/// не должно убить процесс (double panic в hook = abort).
fn install_panic_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<не-строковый payload>".to_string());
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<неизвестно>".to_string());
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tracing::error!(
                    "PANIC в потоке {:?}: {} ({})",
                    std::thread::current().name(),
                    payload,
                    location
                );
            }));
            prev(info);
        }));
    });
}

pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        install_panic_hook();
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_else(|e| format!("ошибка: {e}"));
        tracing::info!(
            "mush-jni: построение runtime: worker_threads=2, available_parallelism={parallelism}"
        );
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("mush-jni")
            // Диагностика пропажи воркера (§3.5 п.15): старт/остановка
            // каждого worker-потока теперь видны в logcat.
            .on_thread_start(|| {
                tracing::info!("mush-jni: runtime worker up");
            })
            .on_thread_stop(|| {
                tracing::warn!("mush-jni: runtime worker ОСТАНОВЛЕН");
            })
            .build()
            .expect("tokio runtime")
    })
}

/// block_on, безопасный при reentrant-вызове из callback'а runtime'а.
///
/// JNI-callback'и (onHandshakeComplete/onMessage/…) исполняются НА воркер-
/// потоке tokio (dispatcher-задача, attach_current_thread, §5.3). Kotlin-
/// callback может синхронно дёрнуть обратно любой Native.* — и тогда
/// `Runtime::block_on` внутри runtime-контекста паникует "Cannot start a
/// runtime from within a runtime", убивая воркер бесшумно (stderr в logcat
/// не идёт): MUSH-HARDWARE-TEST-PLAN §3.5 п.15. При обнаружении runtime-
/// блокируется до результата; прогресс обеспечивают остальные воркеры.
pub fn block_on_reentrant<F>(future: F) -> F::Output
where
    F: Future,
    F::Output: Send,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return runtime().block_on(future);
    }
    // Future наши (raw-указатель на JniNode внутри) формально !Send — для
    // переноса на чужой поток достаточно unsafe-обёртки: указатель живёт
    // весь вызов, владение нодой не передаётся (контракт handle, §5.1).
    // Scoped-thread (не blocking-пул): future может заимствовать (!'static,
    // см. node_import_state) — lifetime гарантирован scope-джойном.
    struct SendFuture<F>(F);
    unsafe impl<F> Send for SendFuture<F> {}
    impl<F> SendFuture<F> {
        // Вызов через метод: closure захватывает обёртку целиком (иначе
        // disjoint capture edition 2021 тянет F напрямую, мимо unsafe-Send).
        fn into_inner(self) -> F {
            self.0
        }
    }
    let future = SendFuture(future);
    let rt = runtime();
    std::thread::scope(|s| {
        let handle = s.spawn(move || rt.block_on(future.into_inner()));
        // Исполнение на честном потоке вне runtime-контекста; паника future
        // всплывёт в join (catch_unwind JNI-входа её и оформит).
        handle
            .join()
            .expect("block_on_reentrant: поток исполнения умер")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Регресс §3.5 п.15: reentrant-вызов (JNI-callback исполняется на
    /// воркере runtime'а и дёргает обратно Native.*) раньше паниковал
    /// "Cannot start a runtime from within a runtime", убивая воркер.
    /// block_on_reentrant обязан вернуть значение и из контекста runtime'а.
    #[test]
    fn block_on_reentrant_inside_runtime_context() {
        runtime().block_on(async {
            let value = block_on_reentrant(async { 42 });
            assert_eq!(value, 42);
        });
    }

    /// Вне runtime-контекста — обычный честный block_on (путь JNI-потоков).
    #[test]
    fn block_on_reentrant_outside_runtime_context() {
        let value = block_on_reentrant(async { 7 });
        assert_eq!(value, 7);
    }
}
