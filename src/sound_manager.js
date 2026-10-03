// ── nanoclick Procedural Sound Engine ───────────────────────────
// Zero-asset Web Audio API acoustic cue synthesizer.
// Synthesizes crystal-clear audio feedback without any external audio files.

// Suppress native context menu on sound engine context if ever called.
window.addEventListener("contextmenu", (e) => e.preventDefault(), true);

const SoundManager = {
  _audioCtx: null,

  _getAudioContext() {
    try {
      const AudioCtxClass = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtxClass) return null;
      if (!this._audioCtx) {
        this._audioCtx = new AudioCtxClass();
      }
      if (this._audioCtx.state === "suspended") {
        this._audioCtx.resume().catch(() => {});
      }
      return this._audioCtx;
    } catch (_) {
      return null;
    }
  },

  play(soundCue) {
    if (!soundCue || soundCue === "none") return;
    const ctx = this._getAudioContext();
    if (!ctx) return;

    try {
      const now = ctx.currentTime;

      switch (soundCue) {
        case "start": {
          // Affirmative 2-tone upward chime (C5 -> E5)
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.type = "sine";
          osc.frequency.setValueAtTime(523.25, now);
          osc.frequency.exponentialRampToValueAtTime(659.25, now + 0.08);
          gain.gain.setValueAtTime(0.25, now);
          gain.gain.exponentialRampToValueAtTime(0.01, now + 0.16);
          osc.connect(gain);
          gain.connect(ctx.destination);
          osc.start(now);
          osc.stop(now + 0.16);
          break;
        }

        case "stop": {
          // Soft damping woodblock tone (E4 -> A3)
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.type = "sine";
          osc.frequency.setValueAtTime(329.63, now);
          osc.frequency.exponentialRampToValueAtTime(220.0, now + 0.1);
          gain.gain.setValueAtTime(0.2, now);
          gain.gain.exponentialRampToValueAtTime(0.01, now + 0.14);
          osc.connect(gain);
          gain.connect(ctx.destination);
          osc.start(now);
          osc.stop(now + 0.14);
          break;
        }

        case "success": {
          // Ascending major arpeggio: C5 (523Hz) -> E5 (659Hz) -> G5 (784Hz)
          [
            { freq: 523.25, delay: 0.00, dur: 0.09 },
            { freq: 659.25, delay: 0.07, dur: 0.09 },
            { freq: 783.99, delay: 0.14, dur: 0.14 }
          ].forEach((note) => {
            const osc = ctx.createOscillator();
            const gain = ctx.createGain();
            osc.type = "sine";
            osc.frequency.setValueAtTime(note.freq, now + note.delay);
            gain.gain.setValueAtTime(0.22, now + note.delay);
            gain.gain.exponentialRampToValueAtTime(0.01, now + note.delay + note.dur);
            osc.connect(gain);
            gain.connect(ctx.destination);
            osc.start(now + note.delay);
            osc.stop(now + note.delay + note.dur);
          });
          break;
        }

        case "warning": {
          // Dual-tone dissonant alert chime (D5 587Hz -> A5 880Hz + triangle overtone)
          const osc1 = ctx.createOscillator();
          const gain1 = ctx.createGain();
          osc1.type = "sine";
          osc1.frequency.setValueAtTime(587.33, now);
          osc1.frequency.exponentialRampToValueAtTime(880.0, now + 0.12);
          gain1.gain.setValueAtTime(0.28, now);
          gain1.gain.exponentialRampToValueAtTime(0.01, now + 0.35);
          osc1.connect(gain1);
          gain1.connect(ctx.destination);
          osc1.start(now);
          osc1.stop(now + 0.35);

          const osc2 = ctx.createOscillator();
          const gain2 = ctx.createGain();
          osc2.type = "triangle";
          osc2.frequency.setValueAtTime(440.0, now + 0.12);
          osc2.frequency.exponentialRampToValueAtTime(659.25, now + 0.28);
          gain2.gain.setValueAtTime(0.18, now + 0.12);
          gain2.gain.exponentialRampToValueAtTime(0.01, now + 0.40);
          osc2.connect(gain2);
          gain2.connect(ctx.destination);
          osc2.start(now + 0.12);
          osc2.stop(now + 0.40);
          break;
        }

        case "emergency": {
          // Urgent descending alert sweep (880Hz -> 330Hz)
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.type = "sawtooth";
          osc.frequency.setValueAtTime(880.0, now);
          osc.frequency.exponentialRampToValueAtTime(329.63, now + 0.22);
          gain.gain.setValueAtTime(0.25, now);
          gain.gain.exponentialRampToValueAtTime(0.01, now + 0.25);
          osc.connect(gain);
          gain.connect(ctx.destination);
          osc.start(now);
          osc.stop(now + 0.25);
          break;
        }

        case "tick": {
          // Subtle micro-blip (800Hz, 30ms) for speed changes and counts
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.type = "sine";
          osc.frequency.setValueAtTime(800.0, now);
          gain.gain.setValueAtTime(0.12, now);
          gain.gain.exponentialRampToValueAtTime(0.01, now + 0.035);
          osc.connect(gain);
          gain.connect(ctx.destination);
          osc.start(now);
          osc.stop(now + 0.035);
          break;
        }

        default:
          break;
      }
    } catch (_) {}
  }
};

window.SoundManager = SoundManager;
