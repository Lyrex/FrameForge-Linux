use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};
use tracing::{error, info, warn};

use crate::app_state::AppState;
use crate::monitor::{append_to_file, now_hms};
use crate::wfcd::RelicReward;
use crate::{db, events, log_parser, log_watcher};

/// Shared state for the reward watcher thread.
pub(crate) struct RewardWatcherDeps {
    pub app: tauri::AppHandle,
    pub flag: Arc<AtomicBool>,
    pub auto_capture_dir: std::path::PathBuf,
}

/// Spawn the single EE.log tailer thread, started once at app startup and running
/// for the app's lifetime. Every handler runs whether or not the memory scanner is
/// on, except the relic-reward OCR trigger, which fires only while `flag`
/// (`monitor_active`) is set.
pub(crate) fn spawn_reward_watcher_thread(deps: RewardWatcherDeps) {
    let RewardWatcherDeps { app, flag, auto_capture_dir } = deps;

    let (session_log_path, last_found_path, blob_sync_pending) = {
        let state = app.state::<AppState>();
        (
            state.overlay_log.clone(),
            state.roots.state.join("frameforge_last_reward.txt"),
            state.blob_sync_pending.clone(),
        )
    };

    let ee_log_path = log_parser::watched_log_path();

    // The gate only makes blob capture faster; with no log to tail the monitor
    // still escalates on its own interval. A missing gate therefore changes no
    // behaviour, so it is reported once at startup.
    info!(
        target: "frameforge::blob_capture",
        gate = if ee_log_path.is_some() { "armed" } else { "disarmed (no EE.log)" },
        "inventory-sync marker gate"
    );
    let Some(log_path) = ee_log_path else { return };
    if !log_path.is_file() {
        warn!(path = %log_path.display(), "EE.log not found; log-driven features stay idle until it appears");
    }

    // Shared flag: true while the reward screen is active according to EE.log
    let reward_screen_active = Arc::new(AtomicBool::new(false));

    // Unix-ms timestamp of the last relic-rewards emit with real items. Zero = never.
    // Written by the OCR task when it locks and emits; read by the dismiss handler to
    // enforce a minimum overlay display time so fast EE.log flushes don't hide the
    // overlay before the user has time to read it.
    let rewards_emitted_ms: Arc<std::sync::atomic::AtomicU64> =
        Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Shared squad size: updated by EE.log watcher when VoidProjections sequence
    // completes, read by OCR loop for each attempt. This lets late-arriving squad
    // data (VoidProjections often arrives 1-2 s after the screen opens) inform
    // subsequent OCR retries so the card count is always correct.
    let shared_squad_size: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));

    // Squad member names collected from EE.log "AddSquadMember:" lines.
    // Passed to OCR so it can reject any text that fuzzy-matches a player name.
    let shared_squad_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    std::thread::spawn(move || {
        log_watcher::seed_ee_log_names(&log_path, &shared_squad_names, &app);

        // Arbitration runs finished while FrameForge was closed are still in
        // the log, so the recorder gets the file from the top before the tail
        // below starts. It keeps its parser afterwards, which is also what
        // lets a run already under way at startup end correctly.
        //
        // Every other handler skips this first read: replaying an old log
        // would fire reward overlays and trade prompts for missions the
        // player finished hours ago.
        let mut arbitration_runs = db::ArbitrationRecorder::default();
        let mut tail = log_parser::LogTail::from_start(log_path);
        if let Some(backfill) = tail.read() {
            log_watcher::record_arbitration_runs(&app, &mut arbitration_runs, backfill.text, false);
        }

        let mut active_since: Option<std::time::Instant> = None;
        // Cooldown: don't fire riven-screen-open again within 4 seconds of the last fire.
        let mut last_riven_fire: Option<std::time::Instant> = None;
        // Cooldown: prevent spawning multiple relic-pick OCR threads if the trigger fires rapidly.
        let mut last_relic_pick_trigger: Option<std::time::Instant> = None;
        // Rolling raw log text used to reconstruct multi-read trade dialogs.
        let mut trade_buffer = String::new();

        // ── VoidProjections reward sequence state ─────────────────────────
        // The game logs squad reward info BEFORE the screen trigger fires.
        // We accumulate it across poll iterations so it's ready when OCR starts.
        let mut vp_state = log_watcher::VoidProjectionState::default();
        // Cooldown: after any dismiss, block new triggers for 5 s to filter
        // stale EE.log lines that can arrive shortly after a dismiss.
        let mut last_dismiss_at: Option<std::time::Instant> = None;
        // ── Relic prefilter ───────────────────────────────────────────────────
        // Projection paths collected from "Resource load completed" EE.log lines
        // while squad loadouts download. Used at trigger time to narrow the OCR
        // candidate list from ~700 items to the ~6-24 rewards of the active relics.
        let mut session_relics: Vec<String> = Vec::new();
        // One diagnostics folder per trigger→dismiss cycle.
        // Created at trigger, BMP written after overlay confirmed, session log at dismiss.
        let diag_arc: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));

        // ponytail: polling; switch to inotify if wake-up latency matters.
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let Some(chunk) = tail.read() else { continue };
            if chunk.restarted {
                // A new launch's log. No run or reward handshake survives it,
                // the relics were downloaded for missions already played, and
                // a half line held over from the old file would otherwise be
                // glued onto the new file's boot-time header.
                arbitration_runs = db::ArbitrationRecorder::default();
                trade_buffer.clear();
                vp_state = log_watcher::VoidProjectionState::default();
                session_relics.clear();
                last_dismiss_at = None;
            }
            let restarted = chunk.restarted;
            let buf = chunk.text;

            // A panic in one handler must not end the thread and with it every
            // log-driven feature. The tail has already moved past this chunk,
            // so the chunk that panicked is lost.
            let handled = catch_unwind(AssertUnwindSafe(|| {
                log_watcher::note_inventory_sync(&buf, &blob_sync_pending);

                // A replaced log arrives whole and may hold runs finished long
                // before this read; those are backfill too.
                //
                // ponytail: a stall of this thread while the game keeps writing
                // the same file also hands over old runs, and they count as live.
                // Nothing in practice stalls it for the length of a mission; gate
                // on the run's own wall clock if that changes.
                log_watcher::record_arbitration_runs(&app, &mut arbitration_runs, buf.clone(), !restarted);

                let lower = buf.to_lowercase();

                log_watcher::collect_void_projection_state(
                    &buf,
                    &mut vp_state,
                    &shared_squad_size,
                    &session_log_path,
                );

                // Relics are announced before the reward screen opens, narrowing OCR candidates.
                log_watcher::collect_session_relics(&buf, &mut session_relics);

                // AddSquadMember, avatar changes and local login all feed the OCR filter.
                log_watcher::collect_ee_log_names(&buf, &shared_squad_names, &app);

                // ── WFM trade whisper detection ──────────────────────────────────
                if lower.contains("(warframe.market)") {
                    log_watcher::parse_and_emit_wfm_whisper(&app, &buf);
                }

                log_watcher::handle_riven_events(&app, &lower, &mut last_riven_fire);
                log_watcher::handle_relic_pick_events(&app, &lower, &mut last_relic_pick_trigger);
                log_watcher::handle_trade_completion(&app, &buf, &mut trade_buffer);

                // Unveil: riven challenge completion
                if lower.contains("modreveal") || (lower.contains("riven") && lower.contains("unveiled")) {
                    let _ = app.emit(events::RIVEN_UNVEILED, ());
                }

                // Trigger: "VoidProjections: GetVoidProjectionReward[s]" fires when the
                // server actually delivers the reward choices to the client — later than
                // the old "initialized" / "openvoidprojectionrewardscreen" lines, which
                // fired before the cards were visible in endless missions.
                // Matching the singular prefix catches both "Reward" and "Rewards" variants.
                //
                // The completed sequence is consumed on every chunk, so one that
                // finished while the monitor was off cannot fire once it is on.
                let sequence_completed = vp_state.consume_sequence_completed();
                let has_trigger = lower.contains("voidprojections: getvoidprojectionreward")
                    || sequence_completed;

                let has_dismiss = log_watcher::dismiss_relic_rewards(
                    &app,
                    &buf,
                    &session_log_path,
                    &diag_arc,
                    &reward_screen_active,
                    &rewards_emitted_ms,
                    log_watcher::DismissState {
                        active_since: &mut active_since,
                        last_dismiss_at: &mut last_dismiss_at,
                        session_relics: &mut session_relics,
                        projection_state: &mut vp_state,
                    },
                );

                // active_since.is_none() guards against duplicate triggers: multiple
                // matching lines (e.g. "Client has reward info" + "relic rewards
                // initialized" 250 ms later) can arrive in consecutive reads while the
                // same reward screen is still open. A second OCR task would emit
                // different card positions and make the overlay stutter.
                let trigger_allowed = !has_dismiss
                    && active_since.is_none()
                    && last_dismiss_at.is_none_or(|t| t.elapsed().as_secs() >= 5)
                    && flag.load(Ordering::SeqCst);
                if has_trigger && trigger_allowed {
                    reward_screen_active.store(true, Ordering::SeqCst);
                    active_since = Some(std::time::Instant::now());

                    log_watcher::prepare_reward_trigger(
                        &app,
                        &shared_squad_names,
                        &shared_squad_size,
                        &session_relics,
                    );

                    // Find the exact EE.log line that matched so we can log it
                    let trigger_line = buf.lines()
                        .find(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("voidprojections: getvoidprojectionreward")
                        })
                        .unwrap_or("<unknown trigger line>")
                        .trim()
                        .to_string();

                    let ts0 = now_hms();

                    // Read at trigger time so a catalogue refresh since startup is used.
                    let full_catalog = {
                        let state = app.state::<AppState>();
                        let relic_rewards = state.relic_rewards.lock().unwrap_or_else(|e| e.into_inner());
                        Arc::new(full_reward_catalog(&relic_rewards))
                    };
                    let (filtered_cat, prefilter_log) = log_watcher::build_relic_reward_catalog(
                        &app,
                        &session_relics,
                        &full_catalog,
                    );
                    log_watcher::prepare_reward_session(
                        &session_log_path,
                        &shared_squad_names,
                        log_watcher::RewardTrigger {
                            timestamp: &ts0,
                            trigger_line: &trigger_line,
                            prefilter_log: &prefilter_log,
                            catalog_len: filtered_cat.len(),
                        },
                        app.state::<AppState>().auto_capture_enabled.load(Ordering::SeqCst)
                            .then_some(auto_capture_dir.as_path()),
                        &diag_arc,
                        &last_found_path,
                    );

                    let _ = app.emit(events::FF_STATUS, "🔍 Relic reward screen detected");
                    // Tell App.tsx to pre-create the overlay window NOW, before OCR finishes.
                    // Window creation takes 1-2 s; pre-creating shaves that off the visible delay.
                    let _ = app.emit(events::RELIC_TRIGGER, ());

                    spawn_reward_ocr(RewardOcrTask {
                        app: app.clone(),
                        cat: filtered_cat,
                        fallback_cat: full_catalog,
                        lpath: last_found_path.clone(),
                        slog: session_log_path.clone(),
                        active: reward_screen_active.clone(),
                        emitted_ms: rewards_emitted_ms.clone(),
                        squad_arc: Arc::clone(&shared_squad_size),
                        names_arc: Arc::clone(&shared_squad_names),
                        diag_arc: Arc::clone(&diag_arc),
                    });
                }

                log_watcher::auto_dismiss_relic_rewards(
                    &app,
                    &session_log_path,
                    &diag_arc,
                    &reward_screen_active,
                    &mut active_since,
                    &mut last_dismiss_at,
                );
            }));
            if handled.is_err() {
                error!("EE.log chunk handler panicked; the chunk was skipped");
            }
        }
    });
}

struct RewardOcrTask {
    app: tauri::AppHandle,
    cat: Arc<Vec<(String, String)>>,
    /// Full catalog, used after 3 no-match attempts on the relic prefilter.
    fallback_cat: Arc<Vec<(String, String)>>,
    lpath: std::path::PathBuf,
    slog: std::path::PathBuf,
    active: Arc<AtomicBool>,
    emitted_ms: Arc<std::sync::atomic::AtomicU64>,
    squad_arc: Arc<Mutex<Option<usize>>>,
    names_arc: Arc<Mutex<Vec<String>>>,
    diag_arc: Arc<Mutex<Option<std::path::PathBuf>>>,
}

fn spawn_reward_ocr(task: RewardOcrTask) {
    let RewardOcrTask {
        app, cat, fallback_cat, lpath, slog, active, emitted_ms, squad_arc, names_arc, diag_arc,
    } = task;
    tauri::async_runtime::spawn(async move {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(45);
        log_watcher::wait_for_squad_hint(&squad_arc).await;

        // Allow the catalog to be rebuilt inside the loop — it may be empty
        // when the trigger fired before WFCD data finished loading.
        let mut cat = cat;
        let mut no_match_streak = 0u32;
        let mut attempt = 0u32;
        let mut screenshot_saved = false;
        let mut best_item_count = 0usize;
        let mut best_payload: Option<serde_json::Value> = None; // locked when complete
        let mut best_low_confidence = false;
        // When no EE squad hint is available, the first "complete" result may
        // undercount cards (e.g. dark text hides a 2-line item name).
        // soft_complete_at tracks the first attempt that returned complete-without-hint
        // so we do one extra retry before locking.
        let mut soft_complete_at: Option<usize> = None;
        // Item count at the time soft_complete_at was set.
        // If the follow-up attempt finds no more items, emit best_payload even if
        // a newly-arrived EE hint raised estimated_cards above the count we saw.
        // (Warframe can show fewer unique cards than squad size when players share
        // the same relic reward — one player lacking reactant is another example.)
        let mut soft_complete_count: usize = 0;
        loop {
            attempt += 1;
            // Rebuild catalog if WFCD hadn't loaded when this OCR session started.
            // Runs only while cat is empty — once populated it stays populated.
            if cat.is_empty() {
                if let Some(fallback) = log_watcher::build_fallback_reward_catalog(&app) {
                    cat = fallback;
                }
            }
            let _ = app.emit(events::FF_STATUS, "📷 OCR scanning...");
            let result = log_watcher::capture_reward_items(
                &app,
                Arc::clone(&cat),
                Arc::clone(&squad_arc),
                Arc::clone(&names_arc),
            ).await;
            if result.is_some() && !screenshot_saved {
                log_watcher::save_reward_screenshot(&app, &diag_arc);
                screenshot_saved = true;
            }
            // Re-read hint for confirm_ready logic below (same mutex, post-capture value).
            let hint_squad = squad_arc.lock().ok().and_then(|g| *g);

            let ts = now_hms();
            let sleep_ms = match &result {
                // ✅ 1+ items found (solo=1, duo=2, trio=3, full squad=4)
                Some((complete, low_confidence, ref items, ref positions, ref dbg)) if !items.is_empty() => {
                    no_match_streak = 0;
                    let payload = Some(serde_json::json!({
                        "items": items, "positions": positions
                    }));

                    // Determine whether this complete result should be locked now.
                    // If we have an EE squad hint the count is authoritative.
                    // If we don't, wait 3 retries (≈1.2 s) before confirming —
                    // the VoidProjections EE.log sequence typically arrives 1–2 s
                    // after the trigger, and we need it before we can validate the
                    // card count. Waiting 3 extra attempts gives it time to arrive.
                    let soft_retries_done = soft_complete_at
                        .is_some_and(|sa| (attempt as usize).saturating_sub(sa) >= 3);
                    // If the EE hint just arrived saying the squad is LARGER than
                    // what we matched, suppress confirmation and keep retrying.
                    // The next pass will use word_card_count = hint_squad, split
                    // the columns correctly, and find the missing card.
                    let hint_wants_more = hint_squad
                        .is_some_and(|h| h > items.len());
                    let confirm_ready = !hint_wants_more
                        && (hint_squad.is_some() || soft_retries_done)
                        && (!*low_confidence || soft_retries_done);

                    // Save best result; only emit to overlay when confirmed (LOCK).
                    // Partial updates are intentionally suppressed — emitting
                    // partial data while the user is still hovering cards causes
                    // the overlay to flicker with wrong items between attempts.
                    // A confident read of as many cards replaces a low-confidence
                    // one, or the retries the gate above waits for could never
                    // change what gets shown.
                    let is_new_best = items.len() > best_item_count
                        || (items.len() == best_item_count && best_low_confidence && !*low_confidence);
                    if is_new_best {
                        best_item_count = items.len();
                        best_payload = payload.clone();
                        best_low_confidence = *low_confidence;
                        log_watcher::log_reward_best_result(
                            &app,
                            log_watcher::RewardAttempt {
                                attempt, ts: &ts, items, dbg,
                            },
                            *complete, confirm_ready,
                            log_watcher::RewardPaths {
                                session_log_path: &slog,
                                last_path: &lpath,
                            },
                        );
                    }

                    // Stop retrying and emit ONLY when all expected cards found AND confirmed.
                    if *complete {
                        if confirm_ready {
                            // Hard cutoff: if dismiss arrived while OCR was running, drop the result.
                            if !active.load(Ordering::SeqCst) { break; }
                            if !is_new_best {
                                log_watcher::log_reward_confirm_no_improvement(
                                    attempt, &ts, items, &slog,
                                );
                            }
                            let _ = append_to_file(&slog, "[STEP 3] OVERLAY OPENED\n\n");
                            // Always emit the BEST result captured so far, not the
                            // current attempt — later attempts may have worse OCR
                            // quality (player-name pollution, brightness change).
                            let emit_val = if best_payload.is_some() { &best_payload } else { &payload };
                            log_watcher::publish_relic_rewards(&app, emit_val.as_ref());
                            emitted_ms.store(
                                std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis() as u64)
                                    .unwrap_or(0),
                                Ordering::SeqCst,
                            );
                            log_watcher::schedule_reward_safety_cleanup(
                                app.clone(),
                                slog.clone(),
                                Arc::clone(&diag_arc),
                            );
                            break;
                        } else if soft_complete_at.is_none() {
                            // Only the first soft-complete attempt is recorded.
                            // Overwriting it later would reset the retry counter every loop.
                            soft_complete_at = Some(attempt as usize);
                            soft_complete_count = best_item_count;
                        }
                    } else if soft_complete_at.is_some() && items.len() <= soft_complete_count {
                        // Soft-complete confirmation retry found no more items.
                        // A late EE hint may have raised estimated_cards above what
                        // the screen actually shows (e.g. squad=4 but only 3 unique
                        // cards because one player lacked reactant or shared a reward).
                        // Emit best_payload now rather than retrying until timeout.
                        if !active.load(Ordering::SeqCst) { break; }
                        let emit_val = best_payload.clone().unwrap_or(serde_json::Value::Null);
                        log_watcher::publish_relic_rewards(&app, Some(&emit_val));
                        let _ = append_to_file(&slog,
                            "[STEP 3] OVERLAY OPENED (soft-complete confirmed — no improvement)\n\n");
                        log_watcher::schedule_reward_safety_cleanup(
                            app.clone(),
                            slog.clone(),
                            Arc::clone(&diag_arc),
                        );
                        break;
                    }
                    // Partial result (or soft-complete pending confirmation) — retry
                    400u64
                }
                // ❌ Text found but no catalog match
                Some((_, _, ref items, _, ref dbg)) => {
                    log_watcher::log_reward_no_match(
                        &app,
                        log_watcher::RewardAttempt {
                            attempt, ts: &ts, items, dbg,
                        },
                        &mut no_match_streak, &mut cat, &fallback_cat,
                        log_watcher::RewardPaths {
                            session_log_path: &slog,
                            last_path: &lpath,
                        },
                    )
                }
                // ⚠️ Warframe window not found
                None => {
                    log_watcher::log_reward_capture_failed(&app, attempt, &ts, &slog, &lpath)
                }
            };

            if std::time::Instant::now() >= deadline {
                log_watcher::finalize_reward_ocr_timeout(
                    &app,
                    best_payload,
                    &active,
                    &slog,
                    &diag_arc,
                );
                break;
            }
            if !active.load(Ordering::SeqCst) {
                log_watcher::log_reward_ocr_stopped(&slog);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }
    });
}

/// Build the OCR catalog from relic rewards.
/// Two keys per relic (uniqueName + display name); dedup by unique_name.
fn full_reward_catalog(
    relic_rewards: &HashMap<String, Vec<RelicReward>>,
) -> Vec<(String, String)> {
    let mut catalog_pairs: Vec<(String, String)> = relic_rewards
        .values()
        .flat_map(|rewards| rewards.iter())
        .filter(|r| !r.name.is_empty())
        .map(|r| (r.unique_name.clone(), r.name.clone()))
        .collect();
    catalog_pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    catalog_pairs.dedup_by(|a, b| !a.0.is_empty() && a.0 == b.0);
    catalog_pairs
}
