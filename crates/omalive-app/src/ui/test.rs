//! The voice test: record ten seconds of the raw microphone and Omalive Mic
//! side by side, measure them against podcast targets, and play either take
//! back.
use super::{Omalive, kit, kit::palette};
use crate::{model::Tone, visuals::Recording};
use gpui_kit::{AnyElement, Context, div, prelude::*, rems};
use omalive_core::voice_test::{self, Assessment, Verdict};
use std::{
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
pub enum VoiceTest {
    #[default]
    Idle,
    Recording(Instant),
    Done(Assessment),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Take {
    Processed,
    Original,
}

impl Take {
    fn file(self) -> &'static str {
        match self {
            Take::Processed => "processed.wav",
            Take::Original => "original.wav",
        }
    }
}

/// The running playback, shared with the task that waits for it to end.
pub struct Playback {
    pub take: Take,
    child: Arc<Mutex<Child>>,
}

/// Where test takes are written: private to this session.
pub fn directory() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("omalive-test")
}

impl Omalive {
    pub(crate) fn start_test(&mut self, cx: &mut Context<Self>) {
        if !self.model.voice_running() || self.model.muted() {
            self.model.say(
                "Start the mic (and unmute) to record a test.",
                Tone::Warning,
            );
            cx.notify();
            return;
        }
        self.stop_playback();
        self.go(super::Page::Voice, cx);
        if let Some(taps) = self.engine.taps() {
            self.visuals.resync(&taps);
        }
        self.visuals.recording = Some(Recording::default());
        self.test = VoiceTest::Recording(Instant::now());
        self.model.say(
            "Recording a 10 second test. Talk as you would on a call.",
            Tone::Neutral,
        );
        cx.notify();
    }

    /// Called each frame while the voice page runs: finish the test once
    /// it has enough audio. `live` is false when the meter stopped.
    pub(crate) fn advance_test(&mut self, live: bool) {
        let VoiceTest::Recording(_) = self.test else {
            return;
        };
        if !live {
            self.visuals.recording = None;
            self.test = VoiceTest::Idle;
            self.model
                .say("The test stopped with the mic.", Tone::Warning);
            return;
        }
        let want = voice_test::RATE * voice_test::SECONDS;
        let Some(recording) = &self.visuals.recording else {
            return;
        };
        if recording.input.len() < want || recording.output.len() < want {
            return;
        }
        let Some(mut recording) = self.visuals.recording.take() else {
            return;
        };
        recording.input.truncate(want);
        recording.output.truncate(want);
        let assessment = voice_test::assess(&recording.input, &recording.output);
        let dir = directory();
        let saved = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(
                dir.join(Take::Original.file()),
                voice_test::wav(&recording.input, voice_test::RATE as u32),
            )?;
            std::fs::write(
                dir.join(Take::Processed.file()),
                voice_test::wav(&recording.output, voice_test::RATE as u32),
            )
        });
        match saved {
            Ok(()) => self.model.say(
                "Test recorded. Play it back with headphones to hear it as others will.",
                Tone::Good,
            ),
            Err(error) => self
                .model
                .say(format!("The test could not be saved: {error}"), Tone::Bad),
        }
        self.test = VoiceTest::Done(assessment);
    }

    pub(crate) fn play(&mut self, take: Take, cx: &mut Context<Self>) {
        let again = self.playback.as_ref().is_some_and(|p| p.take == take);
        self.stop_playback();
        if again {
            cx.notify();
            return;
        }
        let path = directory().join(take.file());
        match std::process::Command::new("pw-play").arg(&path).spawn() {
            Ok(child) => {
                let child = Arc::new(Mutex::new(child));
                self.playback = Some(Playback {
                    take,
                    child: child.clone(),
                });
                // Clear the button when playback ends by itself.
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(200))
                            .await;
                        let done = child
                            .lock()
                            .map_or(true, |mut c| !matches!(c.try_wait(), Ok(None)));
                        if done {
                            let _ = this.update(cx, |this, cx| {
                                if this
                                    .playback
                                    .as_ref()
                                    .is_some_and(|p| Arc::ptr_eq(&p.child, &child))
                                {
                                    this.playback = None;
                                    cx.notify();
                                }
                            });
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => self
                .model
                .say(format!("Playback needs pw-play: {error}"), Tone::Bad),
        }
        cx.notify();
    }

    pub(crate) fn stop_playback(&mut self) {
        if let Some(playback) = self.playback.take()
            && let Ok(mut child) = playback.child.lock()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The record button, beside Mute; hidden while a test records.
    pub(crate) fn test_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if matches!(self.test, VoiceTest::Recording(_)) {
            return None;
        }
        let label = if matches!(self.test, VoiceTest::Done(_)) {
            "Test again"
        } else {
            "Test 10 s"
        };
        Some(
            kit::button("test", label, "t", cx)
                .on_click(cx.listener(|this, _, _, cx| this.start_test(cx)))
                .into_any_element(),
        )
    }

    /// Progress while recording, then the results; nothing before a test.
    pub(crate) fn test_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mut body = div().flex().flex_col().gap(rems(0.45));
        match &self.test {
            VoiceTest::Idle => {}
            VoiceTest::Recording(started) => {
                let elapsed = started.elapsed().as_secs_f32();
                let progress = (elapsed / voice_test::SECONDS as f32).min(1.);
                let left = (voice_test::SECONDS as f32 - elapsed).max(0.).ceil();
                body = body
                    .mt(rems(0.8))
                    .child(
                        div()
                            .text_color(p.red)
                            .child(format!("● Recording a test · talk normally · {left:.0} s")),
                    )
                    .child(kit::level_bar(progress, None, cx));
            }
            VoiceTest::Done(a) => {
                let loudness = a
                    .loudness
                    .map(|l| format!("{l:.0} LUFS"))
                    .unwrap_or_else(|| "– LUFS".into());
                body = body.mt(rems(0.8)).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(rems(1.))
                        .text_color(p.dim)
                        .child(format!("Test · loudness {loudness}"))
                        .child(format!("peak {:.1} dBFS", a.output_peak))
                        .child(format!("mic peak {:.0} dBFS", a.input_peak)),
                );
                for note in &a.notes {
                    let color = match note.verdict {
                        Verdict::Good => p.grn,
                        Verdict::Warning => p.yel,
                        Verdict::Bad => p.red,
                    };
                    body = body.child(
                        div()
                            .flex()
                            .gap(rems(0.6))
                            .items_start()
                            .child(div().pt(rems(0.3)).child(kit::dot(color, true)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(p.fg)
                                    .child(note.text.clone()),
                            ),
                    );
                }
                let playing = self.playback.as_ref().map(|p| p.take);
                let play = |id: &'static str, take: Take, label: &'static str| {
                    kit::button(
                        id,
                        if playing == Some(take) { "Stop" } else { label },
                        "",
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.play(take, cx)))
                };
                body = body.child(
                    div()
                        .flex()
                        .gap(rems(0.6))
                        .mt(rems(0.3))
                        .child(play("play-processed", Take::Processed, "Play processed"))
                        .child(play("play-original", Take::Original, "Play original")),
                );
            }
        }
        body.into_any_element()
    }
}
