// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event's context.

use core::task;
use std::any::type_name;
use std::pin::Pin;
use std::task::Poll;

use crate::enrichment::{Enrichment, EnrichmentTransfer};

/// Captured sink context (enrichment) for cross-thread transfer.
///
/// Created by [`Sink::transfer_context`](crate::Sink::transfer_context). Apply it on the target
/// thread by calling [`Transfer::apply`].
///
/// See the [Enrichment - cross-thread transfer](crate#transferring-enrichment-across-threads-and-tasks)
/// section for the full workflow.
#[derive(Clone, thread_aware::ThreadAware)]
#[must_use]
pub struct Transfer {
    enrichment: EnrichmentTransfer,
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Transfer {
    pub(crate) fn new(enrichment: EnrichmentTransfer) -> Self {
        Self { enrichment }
    }

    /// Adds extra enrichment that is applied along with the captured context.
    ///
    /// The additional enrichment layers on top of the entries already
    /// captured by [`Transfer`], so it is visible on every thread the
    /// transfer is applied to.
    pub fn with_enrichment(mut self, additional_enrichment: impl Enrichment) -> Self {
        self.enrichment.push(additional_enrichment);
        self
    }

    /// Applies the captured enrichment to the current thread.
    ///
    /// The returned guard keeps the enrichment active for its lifetime and
    /// removes it again when dropped. Takes `&self` so the same transfer can
    /// be applied repeatedly (e.g. once per poll of an attached future).
    #[must_use]
    pub fn apply(&self) -> impl Drop {
        self.enrichment.apply()
    }
}

/// A future wrapper that restores a captured [`Transfer`]
/// on every poll.
///
/// Created by [`EnrichFutureExt::attach`](crate::enrichment::EnrichFutureExt::attach).
/// See the [Enrichment - cross-thread transfer](crate#transferring-enrichment-across-threads-and-tasks)
/// section for details.
#[pin_project::pin_project]
#[must_use]
pub struct Transferred<T> {
    #[pin]
    inner: T,
    context_transfer: Transfer,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Transferred<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<T> Transferred<T> {
    pub(crate) fn new(inner: T, context_transfer: Transfer) -> Self {
        Self { inner, context_transfer }
    }
}

impl<F: Future> Future for Transferred<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.context_transfer.apply();
        this.inner.poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use data_privacy::{DataClass, Sensitive};
    use tick::SimpleClock;

    use crate::Enrichment;
    use crate::enrichment::EnrichFnExt;
    use crate::processing::EventProcessor;
    use crate::sink::{Sink, SinkId};

    const TEST_DC: DataClass = DataClass::new("test", "public");
    const EMPTY_PROCESSORS: Vec<Arc<dyn EventProcessor>> = Vec::new();

    #[derive(Enrichment)]
    struct ServiceCtx {
        service: Sensitive<&'static str>,
    }

    #[derive(Enrichment)]
    struct RequestCtx {
        #[dimension(log = "request.id")]
        request_id: Sensitive<&'static str>,
    }

    #[derive(Enrichment)]
    struct KvCtx {
        k1: Sensitive<&'static str>,
        k2: Sensitive<&'static str>,
    }

    #[derive(Enrichment)]
    struct ThreadCtx {
        thread_test: Sensitive<&'static str>,
    }

    #[derive(Enrichment)]
    struct LibCtx {
        #[dimension(log = "library.version")]
        version: Sensitive<&'static str>,
    }

    #[derive(Enrichment)]
    struct AsyncCtx {
        async_key: Sensitive<&'static str>,
    }

    #[test]
    fn enrichments_stack_and_unwind() {
        let sink = Sink::noop();
        (|| {
            (|| {
                let e = sink.current_enrichments();
                assert_eq!(e.len(), 2);
                assert_eq!(e[0].key().as_str(), "service");
                assert_eq!(e[1].key().as_str(), "request.id");
            })
            .enrich(
                &sink,
                RequestCtx {
                    request_id: Sensitive::new("r-42", TEST_DC),
                },
            )();
            let e = sink.current_enrichments();
            assert_eq!(e.len(), 1);
            assert_eq!(e[0].key().as_str(), "service");
        })
        .enrich(
            &sink,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    #[test]
    fn enrich_many_adds_multiple_entries() {
        let sink = Sink::noop();
        (|| {
            let e = sink.current_enrichments();
            assert_eq!(e.len(), 2);
            assert_eq!(e[0].key().as_str(), "k1");
            assert_eq!(e[1].key().as_str(), "k2");
        })
        .enrich(
            &sink,
            KvCtx {
                k1: Sensitive::new("v1", TEST_DC),
                k2: Sensitive::new("v2", TEST_DC),
            },
        )();
    }

    #[test]
    fn emit_event_noop_without_emitters() {
        // Noop sink -> emit_event must not panic (silent drop).
    }

    #[test]
    fn cross_thread_enrichment_transfer() {
        let sink = Sink::noop();
        (|| {
            let transfer = sink.transfer_context();

            let sink = sink.clone();
            let handle = std::thread::spawn(move || {
                let _guard = transfer.apply();
                let e = sink.current_enrichments();
                assert_eq!(e.len(), 1);
                assert_eq!(e[0].key().as_str(), "thread_test");
            });

            handle.join().unwrap();
        })
        .enrich(
            &sink,
            ThreadCtx {
                thread_test: Sensitive::new("value", TEST_DC),
            },
        )();
    }

    #[test]
    fn enrich_for_targets_specific_emitter() {
        static FETCH: SinkId = SinkId::new("fetch");

        let sink = Sink::noop();
        (|| {
            let all = sink.current_enrichments();
            let fetch_entries: Vec<_> = all.iter().filter(|e| e.target() == Some(FETCH)).collect();
            assert_eq!(fetch_entries.len(), 1);
            assert_eq!(fetch_entries[0].key().as_str(), "library.version");
        })
        .enrich_for(
            &sink,
            FETCH,
            LibCtx {
                version: Sensitive::new("1.0", TEST_DC),
            },
        )();
    }

    /// Async test: enrichment entries set via `.enrich().attach()` are
    /// visible on a different thread when propagated through `Transfer`.
    #[cfg(not(miri))] // miri error: can't call foreign function `CreateIoCompletionPort` on OS `windows`
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_enriched_context_transfer_cross_thread() {
        use crate::enrichment::EnrichFutureExt;

        let sink = Sink::noop();
        let transfer = sink.transfer_context();

        let emitter_inner = sink.clone();
        let enriched = async move {
            let e = emitter_inner.current_enrichments();
            assert_eq!(e.len(), 1, "enrichment should be visible on executor thread");
            assert_eq!(e[0].key().as_str(), "async_key");
        }
        .enrich(
            &sink,
            AsyncCtx {
                async_key: Sensitive::new("v", TEST_DC),
            },
        )
        .attach(transfer);

        // Spawn on the multi-thread runtime so it may execute on a different
        // OS thread than the one that created the future.
        tokio::spawn(enriched).await.unwrap();
    }

    /// Each sink owns its own enrichment slot - entries pushed on one
    /// sink are **not** visible on another.
    #[test]
    fn each_emitter_has_its_own_enrichment_slot() {
        static OTHER: SinkId = SinkId::new("ctx_test_other");

        let source = Sink::noop();
        let observer = Sink::new(OTHER, EMPTY_PROCESSORS, SimpleClock::new_frozen());

        (|| {
            let source_entries = source.current_enrichments();
            let observer_entries = observer.current_enrichments();
            // Push on `source` is visible on `source` only.
            assert_eq!(source_entries.len(), 1);
            assert!(observer_entries.is_empty());
        })
        .enrich(
            &source,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// `.enrich(&composite, ...)` broadcasts the push to every child's
    /// enrichment slot - records dispatched to each child carry the
    /// enrichment, and the entries are popped from every child when the
    /// guard drops.
    #[test]
    fn composite_enrich_broadcasts_to_all_children() {
        static APP: SinkId = SinkId::new("ctx_test_app");
        static AUDIT: SinkId = SinkId::new("ctx_test_audit");

        let app = Sink::new(APP, vec![], SimpleClock::new_frozen());
        let audit = Sink::new(AUDIT, vec![], SimpleClock::new_frozen());
        let composite = Sink::composite([app.clone(), audit.clone()]);

        // Before push: both children have empty slots.
        assert!(app.current_enrichments().is_empty());
        assert!(audit.current_enrichments().is_empty());

        (|| {
            // Inside the scope: both children see the entry.
            let app_entries = app.current_enrichments();
            let audit_entries = audit.current_enrichments();
            assert_eq!(app_entries.len(), 1, "app child should see broadcast");
            assert_eq!(audit_entries.len(), 1, "audit child should see broadcast");
            assert_eq!(app_entries[0].key().as_str(), "service");
            assert_eq!(audit_entries[0].key().as_str(), "service");
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        // After scope exit: both children have empty slots again.
        assert!(app.current_enrichments().is_empty(), "app child should be popped");
        assert!(audit.current_enrichments().is_empty(), "audit child should be popped");
    }

    /// Stacking `.enrich(&composite, ...)` and unwinding restores each
    /// child's slot to its previous head - verifies the compound guard
    /// pops every child correctly when the inner scope exits.
    #[test]
    fn composite_enrich_stacks_and_unwinds() {
        static APP: SinkId = SinkId::new("ctx_test_stack_app");
        static AUDIT: SinkId = SinkId::new("ctx_test_stack_audit");

        #[derive(Enrichment)]
        struct Inner {
            #[dimension(log = "request.id")]
            request_id: Sensitive<&'static str>,
        }

        let app = Sink::new(APP, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let audit = Sink::new(AUDIT, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([app.clone(), audit.clone()]);

        (|| {
            // Outer scope: each child's slot has 1 entry (`service`).
            (|| {
                // Inner scope: each child's slot has 2 entries (service + request.id).
                let app_entries = app.current_enrichments();
                let audit_entries = audit.current_enrichments();
                assert_eq!(app_entries.len(), 2, "app should see both outer + inner");
                assert_eq!(audit_entries.len(), 2, "audit should see both outer + inner");
            })
            .enrich(
                &composite,
                Inner {
                    request_id: Sensitive::new("r-42", TEST_DC),
                },
            )();

            // Inner unwound: each child should be back to 1 entry.
            assert_eq!(app.current_enrichments().len(), 1);
            assert_eq!(audit.current_enrichments().len(), 1);
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        // Outer also unwound: empty.
        assert!(app.current_enrichments().is_empty());
        assert!(audit.current_enrichments().is_empty());
    }

    /// Round-tripping a composite's enrichment through `transfer_context`
    /// and `Transfer::apply` restores each child's chain on the
    /// receiving thread. Slot identity is preserved across `Sink::clone`,
    /// so the captured slots address the same chains in the spawned thread.
    #[test]
    fn composite_transfer_round_trips_each_child() {
        static A: SinkId = SinkId::new("ctx_test_xfer_a");
        static B: SinkId = SinkId::new("ctx_test_xfer_b");

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            // Capture from inside the scope - both children's chains are populated.
            let transfer = composite.transfer_context();

            let a_for_thread = a.clone();
            let b_for_thread = b.clone();
            std::thread::spawn(move || {
                // Spawned thread starts with empty per-thread state.
                assert!(a_for_thread.current_enrichments().is_empty());
                assert!(b_for_thread.current_enrichments().is_empty());

                let _guard = transfer.apply();
                assert_eq!(a_for_thread.current_enrichments().len(), 1, "a restored");
                assert_eq!(b_for_thread.current_enrichments().len(), 1, "b restored");
            })
            .join()
            .unwrap();
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// Children with divergent chains (one populated via `.enrich(&child, …)`,
    /// the other left empty) round-trip without one clobbering the other -
    /// the historical bug at `Sink::transfer_context` prior to the
    /// slot-identity refactor.
    #[test]
    fn composite_transfer_preserves_divergent_children() {
        static A: SinkId = SinkId::new("ctx_test_xfer_div_a");
        static B: SinkId = SinkId::new("ctx_test_xfer_div_b");

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        // Push only on `a`. `b` stays empty.
        (|| {
            assert_eq!(a.current_enrichments().len(), 1);
            assert!(b.current_enrichments().is_empty());

            let transfer = composite.transfer_context();

            let a_for_thread = a.clone();
            let b_for_thread = b.clone();
            std::thread::spawn(move || {
                let _g = transfer.apply();
                assert_eq!(a_for_thread.current_enrichments().len(), 1, "a's divergent chain restored");
                assert!(
                    b_for_thread.current_enrichments().is_empty(),
                    "b stays empty - not clobbered by a's chain"
                );
            })
            .join()
            .unwrap();
        })
        .enrich(
            &a,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// `transfer_context` on a composite with zero children yields an empty
    /// transfer. Applying it on any sink produces no entries.
    #[test]
    fn empty_composite_transfer_context_is_empty() {
        static OBSERVER: SinkId = SinkId::new("ctx_test_xfer_empty_observer");

        let composite = Sink::composite(std::iter::empty());
        let transfer = composite.transfer_context();

        let observer = Sink::new(OBSERVER, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let _replay = transfer.apply();
        assert!(observer.current_enrichments().is_empty());
    }

    /// Composite-of-composites flattens broadcast at push time: a push on
    /// the outer composite reaches every leaf via recursive descent, and
    /// each leaf's slot pops on guard drop.
    #[test]
    fn nested_composite_enrich_broadcasts_to_all_leaves() {
        static A: SinkId = SinkId::new("ctx_test_nested_a");
        static B: SinkId = SinkId::new("ctx_test_nested_b");
        static C: SinkId = SinkId::new("ctx_test_nested_c");

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let c = Sink::new(C, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let inner = Sink::composite([a.clone(), b.clone()]);
        let outer = Sink::composite([inner, c.clone()]);

        (|| {
            assert_eq!(a.current_enrichments().len(), 1);
            assert_eq!(b.current_enrichments().len(), 1);
            assert_eq!(c.current_enrichments().len(), 1);
        })
        .enrich(
            &outer,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        assert!(a.current_enrichments().is_empty());
        assert!(b.current_enrichments().is_empty());
        assert!(c.current_enrichments().is_empty());
    }

    /// Symmetric to `nested_composite_enrich_broadcasts_to_all_leaves`, but
    /// for transfer: capturing on a composite-of-composites flattens to one
    /// `(slot, head)` pair per leaf, and applying restores every leaf.
    #[test]
    fn nested_composite_transfer_round_trips_all_leaves() {
        static A: SinkId = SinkId::new("ctx_test_xfer_nested_a");
        static B: SinkId = SinkId::new("ctx_test_xfer_nested_b");
        static C: SinkId = SinkId::new("ctx_test_xfer_nested_c");

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let c = Sink::new(C, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let inner = Sink::composite([a.clone(), b.clone()]);
        let outer = Sink::composite([inner, c.clone()]);

        (|| {
            let transfer = outer.transfer_context();

            let a_for_thread = a.clone();
            let b_for_thread = b.clone();
            let c_for_thread = c.clone();
            std::thread::spawn(move || {
                let _g = transfer.apply();
                assert_eq!(a_for_thread.current_enrichments().len(), 1, "a restored");
                assert_eq!(b_for_thread.current_enrichments().len(), 1, "b restored");
                assert_eq!(c_for_thread.current_enrichments().len(), 1, "c restored");
            })
            .join()
            .unwrap();
        })
        .enrich(
            &outer,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// `Transfer::with_enrichment` adds an extra enrichment node
    /// onto every captured chain. After applying, each child's chain should
    /// carry both the captured entry and the additional one.
    #[test]
    fn with_enrichment_broadcasts_to_each_captured_chain() {
        static A: SinkId = SinkId::new("ctx_test_with_enr_a");
        static B: SinkId = SinkId::new("ctx_test_with_enr_b");

        #[derive(Enrichment)]
        struct Extra {
            #[dimension(log = "extra")]
            extra: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            let transfer = composite.transfer_context().with_enrichment(Extra {
                extra: Sensitive::new("v", TEST_DC),
            });

            let a_for_thread = a.clone();
            let b_for_thread = b.clone();
            std::thread::spawn(move || {
                let _g = transfer.apply();
                let a_keys: Vec<_> = a_for_thread
                    .current_enrichments()
                    .iter()
                    .map(|e| e.key().as_str().to_owned())
                    .collect();
                let b_keys: Vec<_> = b_for_thread
                    .current_enrichments()
                    .iter()
                    .map(|e| e.key().as_str().to_owned())
                    .collect();
                assert_eq!(a_keys, vec!["service", "extra"]);
                assert_eq!(b_keys, vec!["service", "extra"]);
            })
            .join()
            .unwrap();
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// `Transfer::with_enrichment` on a transfer captured from an
    /// empty composite is a defined no-op - there are no captured chains
    /// to push onto, so the additional enrichment is silently dropped.
    #[test]
    fn with_enrichment_on_empty_composite_transfer_is_noop() {
        static OBSERVER: SinkId = SinkId::new("ctx_test_with_enr_empty_observer");

        #[derive(Enrichment)]
        struct Extra {
            extra: Sensitive<&'static str>,
        }

        let composite = Sink::composite(std::iter::empty());
        let transfer = composite.transfer_context().with_enrichment(Extra {
            extra: Sensitive::new("v", TEST_DC),
        });

        // Apply on a fresh single sink - its slot is not in the transfer,
        // so nothing should land.
        let observer = Sink::new(OBSERVER, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let _g = transfer.apply();
        assert!(observer.current_enrichments().is_empty());
    }

    /// Chaining multiple `with_enrichment` calls on an empty transfer is a
    /// no-op for every call - exercises the early-return short-circuit in
    /// `EnrichmentTransfer::push` (no slots ⇒ nothing to push onto). After
    /// every step in the chain, applying the transfer must leave the
    /// observer's chain empty.
    #[test]
    fn with_enrichment_on_empty_transfer_short_circuits_for_every_push() {
        static OBSERVER: SinkId = SinkId::new("ctx_test_empty_xfer_chain");

        #[derive(Enrichment)]
        struct A {
            a: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct B {
            b: Sensitive<&'static str>,
        }

        let observer = Sink::new(OBSERVER, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let empty = Sink::composite(std::iter::empty()).transfer_context();

        // After one push: still empty.
        let after_one = empty.clone().with_enrichment(A {
            a: Sensitive::new("va", TEST_DC),
        });
        {
            let _g = after_one.apply();
            assert!(observer.current_enrichments().is_empty());
        }

        // After two chained pushes: still empty.
        let after_two = after_one.with_enrichment(B {
            b: Sensitive::new("vb", TEST_DC),
        });
        {
            let _g = after_two.apply();
            assert!(observer.current_enrichments().is_empty());
        }

        // The original empty transfer is itself untouched.
        {
            let _g = empty.apply();
            assert!(observer.current_enrichments().is_empty());
        }
    }

    /// Apply replaces the target thread's slot head only for the lifetime
    /// of the guard. State that already existed on the target thread is
    /// shadowed during the apply scope, then restored on guard drop.
    #[test]
    fn apply_context_transfer_preserves_local_enrichment_on_target_thread() {
        static E: SinkId = SinkId::new("ctx_test_local_preserved");

        #[derive(Enrichment)]
        struct Local {
            local: Sensitive<&'static str>,
        }

        let sink = Sink::new(E, EMPTY_PROCESSORS, SimpleClock::new_frozen());

        // Capture a transfer carrying a `service` entry while it's on the chain.
        let transfer = (|| sink.transfer_context()).enrich(
            &sink,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        // Push a different entry locally, then apply the transfer on top.
        (|| {
            // Local-only state visible before apply.
            let pre = sink.current_enrichments();
            assert_eq!(pre.len(), 1);
            assert_eq!(pre[0].key().as_str(), "local");

            {
                let _g = transfer.apply();
                // During apply scope: captured state replaces local head.
                let inside = sink.current_enrichments();
                assert_eq!(inside.len(), 1);
                assert_eq!(inside[0].key().as_str(), "service");
            }

            // After guard drop: local state restored.
            let post = sink.current_enrichments();
            assert_eq!(post.len(), 1);
            assert_eq!(post[0].key().as_str(), "local");
        })
        .enrich(
            &sink,
            Local {
                local: Sensitive::new("here", TEST_DC),
            },
        )();
    }

    /// Stacking two `Transfer::apply` calls on the same slot unwinds
    /// in LIFO order - inner guard restores to the head captured by the outer guard,
    /// outer guard restores to the original (empty) state.
    #[test]
    fn nested_apply_context_transfer_unwinds_lifo() {
        static E: SinkId = SinkId::new("ctx_test_nested_apply");

        #[derive(Enrichment)]
        struct Outer {
            outer: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Inner {
            inner: Sensitive<&'static str>,
        }

        let sink = Sink::new(E, EMPTY_PROCESSORS, SimpleClock::new_frozen());

        let outer_transfer = (|| sink.transfer_context()).enrich(
            &sink,
            Outer {
                outer: Sensitive::new("o", TEST_DC),
            },
        )();
        let inner_transfer = (|| sink.transfer_context()).enrich(
            &sink,
            Inner {
                inner: Sensitive::new("i", TEST_DC),
            },
        )();

        assert!(sink.current_enrichments().is_empty());

        {
            let _g_outer = outer_transfer.apply();
            let outer_keys: Vec<_> = sink.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
            assert_eq!(outer_keys, vec!["outer"]);

            {
                let _g_inner = inner_transfer.apply();
                let inner_keys: Vec<_> = sink.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                assert_eq!(inner_keys, vec!["inner"]);
            }

            // Inner guard dropped: outer state restored.
            let after_inner: Vec<_> = sink.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
            assert_eq!(after_inner, vec!["outer"]);
        }

        // Outer guard dropped: original empty state restored.
        assert!(sink.current_enrichments().is_empty());
    }

    /// The transfer carries slot identity - applying it via a structurally
    /// different sink mutates the *captured* slots, leaving the target
    /// sink's own slots untouched. Locks in the semantic change from
    /// the slot-identity refactor (no more cross-sink broadcast).
    #[test]
    fn transfer_targets_captured_slots_not_apply_target() {
        static SOURCE: SinkId = SinkId::new("ctx_test_id_source");
        static OBSERVER: SinkId = SinkId::new("ctx_test_id_observer");

        let source = Sink::new(SOURCE, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let observer = Sink::new(OBSERVER, EMPTY_PROCESSORS, SimpleClock::new_frozen());

        // Capture from `source` while it carries an entry; then apply on `observer`.
        let transfer = (|| source.transfer_context()).enrich(
            &source,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        // `source`'s slot is currently empty (the .enrich scope ended).
        assert!(source.current_enrichments().is_empty());
        assert!(observer.current_enrichments().is_empty());

        {
            let _g = transfer.apply();
            // `source`'s slot was the captured one - it gets re-populated.
            assert_eq!(source.current_enrichments().len(), 1);
            assert_eq!(source.current_enrichments()[0].key().as_str(), "service");
            // `observer`'s slot is untouched.
            assert!(observer.current_enrichments().is_empty());
        }

        // Guard drop restores `source` back to empty.
        assert!(source.current_enrichments().is_empty());
    }

    /// Outer scope enriches the composite (broadcast); inner scope enriches
    /// only one child directly. The inner-only entry must be visible *only*
    /// on that child - siblings stay at the outer-broadcast level.
    #[test]
    fn enrich_one_child_inside_composite_scope() {
        static A: SinkId = SinkId::new("ctx_test_layered_a");
        static B: SinkId = SinkId::new("ctx_test_layered_b");

        #[derive(Enrichment)]
        struct ChildOnly {
            child_only: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            // Outer scope: broadcast to both children.
            (|| {
                // Inner scope: push only on `a`.
                let a_keys: Vec<_> = a.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                let b_keys: Vec<_> = b.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                assert_eq!(a_keys, vec!["service", "child_only"]);
                assert_eq!(b_keys, vec!["service"]);
            })
            .enrich(
                &a,
                ChildOnly {
                    child_only: Sensitive::new("v", TEST_DC),
                },
            )();

            // Inner scope unwound: a back to outer-only.
            assert_eq!(a.current_enrichments().len(), 1);
            assert_eq!(b.current_enrichments().len(), 1);
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();

        // All scopes unwound.
        assert!(a.current_enrichments().is_empty());
        assert!(b.current_enrichments().is_empty());
    }

    /// Outer scope enriches a child directly; inner scope enriches the
    /// composite. The composite broadcast layers on top of every child's
    /// chain - including the one already enriched directly. Sibling sees
    /// only the broadcast.
    #[test]
    fn enrich_composite_inside_child_scope() {
        static A: SinkId = SinkId::new("ctx_test_inverted_a");
        static B: SinkId = SinkId::new("ctx_test_inverted_b");

        #[derive(Enrichment)]
        struct ChildOnly {
            child_only: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            // Outer scope: only `a` has `child_only`.
            assert_eq!(a.current_enrichments()[0].key().as_str(), "child_only", "outer state on a");
            assert!(b.current_enrichments().is_empty(), "outer doesn't reach b");

            (|| {
                // Inner scope: composite broadcast layers on top of both children.
                let a_keys: Vec<_> = a.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                let b_keys: Vec<_> = b.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                assert_eq!(a_keys, vec!["child_only", "service"]);
                assert_eq!(b_keys, vec!["service"]);
            })
            .enrich(
                &composite,
                ServiceCtx {
                    service: Sensitive::new("api", TEST_DC),
                },
            )();

            // Inner unwound: a back to child-only, b empty.
            assert_eq!(a.current_enrichments().len(), 1);
            assert!(b.current_enrichments().is_empty());
        })
        .enrich(
            &a,
            ChildOnly {
                child_only: Sensitive::new("v", TEST_DC),
            },
        )();
    }

    /// Stacking two `.enrich(&composite, …)` scopes: every child's chain
    /// carries both entries, in outer-to-inner order.
    #[test]
    fn enrich_composite_twice_stacks_on_each_child() {
        static A: SinkId = SinkId::new("ctx_test_stack_twice_a");
        static B: SinkId = SinkId::new("ctx_test_stack_twice_b");

        #[derive(Enrichment)]
        struct Inner {
            #[dimension(log = "request.id")]
            request_id: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            (|| {
                let a_keys: Vec<_> = a.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                let b_keys: Vec<_> = b.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                assert_eq!(a_keys, vec!["service", "request.id"]);
                assert_eq!(b_keys, vec!["service", "request.id"]);
            })
            .enrich(
                &composite,
                Inner {
                    request_id: Sensitive::new("r-42", TEST_DC),
                },
            )();
        })
        .enrich(
            &composite,
            ServiceCtx {
                service: Sensitive::new("api", TEST_DC),
            },
        )();
    }

    /// Two children, each enriched independently in nested scopes -
    /// neither pushes leak into the other chain.
    #[test]
    fn enrich_each_child_independently_no_crosstalk() {
        static A: SinkId = SinkId::new("ctx_test_indep_a");
        static B: SinkId = SinkId::new("ctx_test_indep_b");

        #[derive(Enrichment)]
        struct AOnly {
            a_only: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct BOnly {
            b_only: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        // Composite exists but we don't push through it - exercises that
        // member-of-composite emitters still have independent slots.
        let _composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            (|| {
                let a_keys: Vec<_> = a.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                let b_keys: Vec<_> = b.current_enrichments().iter().map(|e| e.key().as_str().to_owned()).collect();
                assert_eq!(a_keys, vec!["a_only"]);
                assert_eq!(b_keys, vec!["b_only"]);
            })
            .enrich(
                &b,
                BOnly {
                    b_only: Sensitive::new("vb", TEST_DC),
                },
            )();
        })
        .enrich(
            &a,
            AOnly {
                a_only: Sensitive::new("va", TEST_DC),
            },
        )();
    }

    /// Enriching an sink that is not in the composite leaves the
    /// composite's children untouched, and vice-versa.
    #[test]
    fn enrich_non_member_emitter_isolated_from_composite() {
        static A: SinkId = SinkId::new("ctx_test_nonmember_a");
        static B: SinkId = SinkId::new("ctx_test_nonmember_b");
        static OUTSIDER: SinkId = SinkId::new("ctx_test_nonmember_outsider");

        #[derive(Enrichment)]
        struct Outside {
            outside: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let outsider = Sink::new(OUTSIDER, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let composite = Sink::composite([a.clone(), b.clone()]);

        (|| {
            (|| {
                // Composite broadcast hits a and b only.
                assert_eq!(a.current_enrichments().len(), 1);
                assert_eq!(b.current_enrichments().len(), 1);
                assert_eq!(a.current_enrichments()[0].key().as_str(), "service");
                // Outsider was enriched separately and is unaffected.
                assert_eq!(outsider.current_enrichments().len(), 1);
                assert_eq!(outsider.current_enrichments()[0].key().as_str(), "outside");
            })
            .enrich(
                &composite,
                ServiceCtx {
                    service: Sensitive::new("api", TEST_DC),
                },
            )();

            // Composite scope unwound: outsider still carries its entry.
            assert!(a.current_enrichments().is_empty());
            assert!(b.current_enrichments().is_empty());
            assert_eq!(outsider.current_enrichments().len(), 1);
        })
        .enrich(
            &outsider,
            Outside {
                outside: Sensitive::new("v", TEST_DC),
            },
        )();
    }

    /// Two partially-overlapping composites stacked. `outer = [a, b]` carries
    /// `x`; `inner = [b, c]` carries `y`. The shared child `b` accumulates
    /// both; the unique children see only their composite's entry.
    #[test]
    fn enrich_two_partially_overlapping_composites_stack() {
        static A: SinkId = SinkId::new("ctx_test_overlap_a");
        static B: SinkId = SinkId::new("ctx_test_overlap_b");
        static C: SinkId = SinkId::new("ctx_test_overlap_c");

        #[derive(Enrichment)]
        struct X {
            x: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Y {
            y: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let c = Sink::new(C, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let outer = Sink::composite([a.clone(), b.clone()]);
        let inner = Sink::composite([b.clone(), c.clone()]);

        let keys = |e: &Sink| {
            e.current_enrichments()
                .iter()
                .map(|e| e.key().as_str().to_owned())
                .collect::<Vec<_>>()
        };

        (|| {
            // Outer scope: only `a` and `b` carry `x`.
            assert_eq!(keys(&a), vec!["x"]);
            assert_eq!(keys(&b), vec!["x"]);
            assert!(keys(&c).is_empty());

            (|| {
                // Inner scope: shared child `b` has both; siblings have only theirs.
                assert_eq!(keys(&a), vec!["x"]);
                assert_eq!(keys(&b), vec!["x", "y"]);
                assert_eq!(keys(&c), vec!["y"]);
            })
            .enrich(
                &inner,
                Y {
                    y: Sensitive::new("vy", TEST_DC),
                },
            )();

            // Inner unwound: `y` popped from b and c.
            assert_eq!(keys(&a), vec!["x"]);
            assert_eq!(keys(&b), vec!["x"]);
            assert!(keys(&c).is_empty());
        })
        .enrich(
            &outer,
            X {
                x: Sensitive::new("vx", TEST_DC),
            },
        )();

        // All scopes unwound.
        assert!(keys(&a).is_empty());
        assert!(keys(&b).is_empty());
        assert!(keys(&c).is_empty());
    }

    /// Three layered scopes: composite → single child → composite. `outer`
    /// broadcasts `x` to `[a, b]`; `middle` adds `y` only on `a`; `innermost`
    /// broadcasts `z` to `[a, b, c]`.
    #[test]
    fn enrich_composite_then_child_then_composite() {
        static A: SinkId = SinkId::new("ctx_test_cmc_a");
        static B: SinkId = SinkId::new("ctx_test_cmc_b");
        static C: SinkId = SinkId::new("ctx_test_cmc_c");

        #[derive(Enrichment)]
        struct X {
            x: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Y {
            y: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Z {
            z: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let c = Sink::new(C, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let outer = Sink::composite([a.clone(), b.clone()]);
        let innermost = Sink::composite([a.clone(), b.clone(), c.clone()]);

        let keys = |e: &Sink| {
            e.current_enrichments()
                .iter()
                .map(|e| e.key().as_str().to_owned())
                .collect::<Vec<_>>()
        };

        (|| {
            // Outer: x on a and b.
            assert_eq!(keys(&a), vec!["x"]);
            assert_eq!(keys(&b), vec!["x"]);
            assert!(keys(&c).is_empty());

            (|| {
                // Middle: y added only on a.
                assert_eq!(keys(&a), vec!["x", "y"]);
                assert_eq!(keys(&b), vec!["x"]);
                assert!(keys(&c).is_empty());

                (|| {
                    // Innermost: z broadcast to a, b, c.
                    assert_eq!(keys(&a), vec!["x", "y", "z"]);
                    assert_eq!(keys(&b), vec!["x", "z"]);
                    assert_eq!(keys(&c), vec!["z"]);
                })
                .enrich(
                    &innermost,
                    Z {
                        z: Sensitive::new("vz", TEST_DC),
                    },
                )();

                // Innermost unwound: z popped from a, b, c.
                assert_eq!(keys(&a), vec!["x", "y"]);
                assert_eq!(keys(&b), vec!["x"]);
                assert!(keys(&c).is_empty());
            })
            .enrich(
                &a,
                Y {
                    y: Sensitive::new("vy", TEST_DC),
                },
            )();

            // Middle unwound: y popped from a.
            assert_eq!(keys(&a), vec!["x"]);
            assert_eq!(keys(&b), vec!["x"]);
        })
        .enrich(
            &outer,
            X {
                x: Sensitive::new("vx", TEST_DC),
            },
        )();

        assert!(keys(&a).is_empty());
        assert!(keys(&b).is_empty());
        assert!(keys(&c).is_empty());
    }

    /// Three layered scopes: single child → composite → partially-different
    /// composite. `outer` adds `x` only on `a`; `middle = [a, b]` broadcasts
    /// `y`; `innermost = [b, c]` broadcasts `z` to a *different* set of
    /// children (`a` is not in the innermost).
    #[test]
    fn enrich_child_then_composite_then_overlapping_composite() {
        static A: SinkId = SinkId::new("ctx_test_ccc_a");
        static B: SinkId = SinkId::new("ctx_test_ccc_b");
        static C: SinkId = SinkId::new("ctx_test_ccc_c");

        #[derive(Enrichment)]
        struct X {
            x: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Y {
            y: Sensitive<&'static str>,
        }

        #[derive(Enrichment)]
        struct Z {
            z: Sensitive<&'static str>,
        }

        let a = Sink::new(A, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let b = Sink::new(B, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let c = Sink::new(C, EMPTY_PROCESSORS, SimpleClock::new_frozen());
        let middle = Sink::composite([a.clone(), b.clone()]);
        let innermost = Sink::composite([b.clone(), c.clone()]);

        let keys = |e: &Sink| {
            e.current_enrichments()
                .iter()
                .map(|e| e.key().as_str().to_owned())
                .collect::<Vec<_>>()
        };

        (|| {
            // Outer: x only on a.
            assert_eq!(keys(&a), vec!["x"]);
            assert!(keys(&b).is_empty());
            assert!(keys(&c).is_empty());

            (|| {
                // Middle: y broadcast to a, b.
                assert_eq!(keys(&a), vec!["x", "y"]);
                assert_eq!(keys(&b), vec!["y"]);
                assert!(keys(&c).is_empty());

                (|| {
                    // Innermost: z broadcast to b, c (a NOT in innermost).
                    assert_eq!(keys(&a), vec!["x", "y"], "a unaffected by innermost");
                    assert_eq!(keys(&b), vec!["y", "z"]);
                    assert_eq!(keys(&c), vec!["z"]);
                })
                .enrich(
                    &innermost,
                    Z {
                        z: Sensitive::new("vz", TEST_DC),
                    },
                )();

                // Innermost unwound: z popped from b and c only.
                assert_eq!(keys(&a), vec!["x", "y"]);
                assert_eq!(keys(&b), vec!["y"]);
                assert!(keys(&c).is_empty());
            })
            .enrich(
                &middle,
                Y {
                    y: Sensitive::new("vy", TEST_DC),
                },
            )();

            // Middle unwound: y popped from a, b.
            assert_eq!(keys(&a), vec!["x"]);
            assert!(keys(&b).is_empty());
        })
        .enrich(
            &a,
            X {
                x: Sensitive::new("vx", TEST_DC),
            },
        )();

        assert!(keys(&a).is_empty());
        assert!(keys(&b).is_empty());
        assert!(keys(&c).is_empty());
    }
}
