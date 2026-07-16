#!/usr/bin/env python3
"""Procedural WAV generator for Wave-Q audio assets.

Generates three original sound effects using only the Python
standard library (``wave``, ``math``, ``struct``, ``random``):

- ``assets/audio/positive.wav`` -- a short, bright, *rising* reward chime.
  An ascending major triad with bell-like harmonics and a soft attack /
  exponential decay envelope. Deliberately distinct from ``coin.wav``
  (which is a single stable ~1316 Hz tone): ``positive`` sweeps *upward*
  through C5->E5->G5->C6 (~523->659->784->1047 Hz) with brighter partials.

- ``assets/audio/penalty.wav`` -- a short, low, *downward* dull impact.
  A pitch glide that descends from ~330 Hz to ~110 Hz over ~0.30 s with a
  low onset thud and muted (low-passed) character. Deliberately distinct
  from ``hit.wav`` (which is a single-ish ~1118 Hz tone): ``penalty`` is
  lower in pitch, descends, and is duller.

It also writes ``assets/audio/tire_squeal_loop.wav``: a seamless 0.7-second
band-limited noise loop for hard turns and handbrake drifts. Its periodic
noise spectrum is confined to 1.5--4 kHz, with no stable low tonal component.

All three files are deterministic (seeded), mono, 16-bit PCM, 44100 Hz -- the
same format as the existing six audio assets -- normalized to ~90% of full
scale with hard anti-clipping so they are web-safe and consistent in level.
The script never touches the existing six assets (ambient, click, coin, crash,
engine, hit).

Run::

    python tools/generate_audio.py

The generator is its own provenance: every sample is synthesised from a
fixed random seed and pure math, so re-running produces byte-identical
output. No third-party recordings or samples are used.
"""

from __future__ import annotations

import math
import os
import random
import struct
import wave

# ---------------------------------------------------------------------------
# Shared format constants (match the existing audio assets).
# ---------------------------------------------------------------------------
SAMPLE_RATE = 44100          # Hz
CHANNELS = 1                 # mono
SAMPLE_WIDTH = 2             # 16-bit PCM
# Target peak as a fraction of full scale. Leaves headroom, avoids clipping.
TARGET_PEAK = 0.90

AUDIO_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "assets", "audio"
)

# The existing six assets are owned by other waves; this generator must not
# overwrite them. Assert at write time to be safe.
EXISTING_ASSETS = {
    "ambient.wav",
    "click.wav",
    "coin.wav",
    "crash.wav",
    "engine.wav",
    "hit.wav",
}


# ---------------------------------------------------------------------------
# Low-level synthesis helpers.
# ---------------------------------------------------------------------------
def _adsr_envelope(n: int, attack: float, decay: float, sustain: float,
                   release: float, sustain_level: float, rate: int) -> list[float]:
    """Simple ADSR envelope of length ``n`` (in samples).

    ``attack``/``decay``/``release`` are fractions of the total length;
    ``sustain`` is the remaining fraction. ``sustain_level`` is 0..1.
    """
    env = [0.0] * n
    a = max(1, int(n * attack))
    d = max(1, int(n * decay))
    r = max(1, int(n * release))
    s = max(1, n - a - d - r)
    end_a, end_d, end_s = a, a + d, a + d + s
    for i in range(n):
        if i < end_a:
            env[i] = i / end_a
        elif i < end_d:
            env[i] = 1.0 - (1.0 - sustain_level) * (i - end_a) / (end_d - end_a)
        elif i < end_s:
            env[i] = sustain_level
        else:
            env[i] = sustain_level * (1.0 - (i - end_s) / r)
    return env


def _exp_decay_envelope(n: int, rate_const: float) -> list[float]:
    """Exponential decay envelope: exp(-rate_const * t)."""
    return [math.exp(-rate_const * i / SAMPLE_RATE) for i in range(n)]


def _soft_attack(env: list[float], attack_samples: int) -> list[float]:
    """Multiply the first ``attack_samples`` by a raised-cosine ramp."""
    a = max(1, attack_samples)
    out = env[:]
    for i in range(min(a, len(out))):
        out[i] *= 0.5 * (1.0 - math.cos(math.pi * i / a))
    return out


def _normalize(samples: list[float], target_peak: float = TARGET_PEAK) -> list[float]:
    """Peak-normalise to ``target_peak`` of full scale; no clipping possible."""
    peak = max((abs(s) for s in samples), default=0.0)
    if peak < 1e-9:
        return samples
    gain = target_peak / peak
    return [s * gain for s in samples]


def _to_pcm16(samples: list[float]) -> bytes:
    """Convert float samples (-1..1) to little-endian signed 16-bit PCM bytes."""
    out = bytearray()
    for s in samples:
        v = int(round(max(-1.0, min(1.0, s)) * 32767))
        out += struct.pack("<h", v)
    return bytes(out)


def _write_wav(path: str, samples: list[float]) -> None:
    """Write a mono 16-bit 44100 Hz PCM WAV file at ``path``."""
    name = os.path.basename(path)
    if name in EXISTING_ASSETS:
        raise RuntimeError(
            f"Refusing to overwrite existing asset '{name}'. "
            "This generator only owns positive.wav, penalty.wav, and "
            "tire_squeal_loop.wav."
        )
    os.makedirs(os.path.dirname(path), exist_ok=True)
    pcm = _to_pcm16(_normalize(samples))
    with wave.open(path, "wb") as w:
        w.setnchannels(CHANNELS)
        w.setsampwidth(SAMPLE_WIDTH)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm)


# ---------------------------------------------------------------------------
# positive.wav -- bright rising reward chime.
# ---------------------------------------------------------------------------
def synth_positive() -> list[float]:
    """Ascending major triad (C5->E5->G5->C6) with bell-like harmonics."""
    random.seed(20260712)  # deterministic

    # Note frequencies (Hz). Ascending major triad + octave -- bright & rising.
    notes = [523.25, 659.25, 783.99, 1046.50]  # C5, E5, G5, C6
    # Each note occupies a sequential slot; slight overlap via tails.
    total_dur = 0.46
    n_total = int(total_dur * SAMPLE_RATE)
    slot = n_total // len(notes)

    buf = [0.0] * n_total

    for idx, freq in enumerate(notes):
        start = idx * slot
        # Bell-like partials: fundamental + bright inharmonic-ish partials.
        # Amplitudes decay quickly with partial number for a clean chime.
        partials = [
            (1.0, 1.00),
            (2.0, 0.45),
            (3.0, 0.22),
            (4.01, 0.12),
            (5.03, 0.06),
        ]
        decay = 6.5  # fairly quick bell decay
        for i in range(slot):
            t = i / SAMPLE_RATE
            env = math.exp(-decay * t)
            # soft attack on the very first note only
            if idx == 0 and i < 80:
                env *= 0.5 * (1.0 - math.cos(math.pi * i / 80))
            s = 0.0
            for mult, amp in partials:
                s += amp * math.sin(2.0 * math.pi * freq * mult * t)
            buf[start + i] += s * env * 0.3

    # A short, very soft trailing shimmer (octave above final note) for sparkle.
    shimmer_freq = notes[-1] * 2.0
    sh_start = (len(notes) - 1) * slot
    sh_len = slot
    for i in range(sh_len):
        t = i / SAMPLE_RATE
        env = math.exp(-9.0 * t)
        buf[sh_start + i] += 0.12 * env * math.sin(2.0 * math.pi * shimmer_freq * t)

    return buf


# ---------------------------------------------------------------------------
# penalty.wav -- low downward dull impact.
# ---------------------------------------------------------------------------
def synth_penalty() -> list[float]:
    """Descending pitch glide ~330->110 Hz with a dull low onset thud."""
    random.seed(20260712)  # deterministic

    dur = 0.30
    n = int(dur * SAMPLE_RATE)

    f_start = 330.0  # lower than hit's ~1118 Hz
    f_end = 110.0    # descends downward

    buf = [0.0] * n

    # Phase-accumulated descending glide (keeps continuous phase, no clicks).
    phase = 0.0
    for i in range(n):
        t = i / n  # 0..1 normalized position
        freq = f_start + (f_end - f_start) * t
        phase += 2.0 * math.pi * freq / SAMPLE_RATE
        # Exponential amplitude decay -> a dull thud that tails off.
        env = math.exp(-7.0 * (i / SAMPLE_RATE))
        # Dull: emphasise fundamental, add only a soft 2nd partial (lowpassed feel).
        fundamental = math.sin(phase)
        second = 0.25 * math.sin(2.0 * phase)
        buf[i] = (fundamental + second) * env

    # Onset thud: a brief, low-frequency noise burst, heavily decaying, to give
    # a dull "impact" transient without brightness.
    for i in range(min(int(0.045 * SAMPLE_RATE), n)):
        t = i / SAMPLE_RATE
        env = math.exp(-55.0 * t)  # very fast decay
        # Low-frequency pseudo-noise: sum of a few slow sinusoids (deterministic).
        noise = (
            0.6 * math.sin(2.0 * math.pi * 90.0 * t)
            + 0.4 * math.sin(2.0 * math.pi * 60.0 * t + 1.3)
            + 0.3 * math.sin(2.0 * math.pi * 45.0 * t + 2.7)
        )
        buf[i] += noise * env * 0.5

    # Soft attack ramp to remove any click at sample 0.
    ramp = 24
    for i in range(min(ramp, n)):
        buf[i] *= i / ramp

    return buf


# ---------------------------------------------------------------------------
# tire_squeal_loop.wav -- seamless, band-limited tire noise.
# ---------------------------------------------------------------------------
def synth_tire_squeal_loop() -> list[float]:
    """Build periodic noise using only Fourier bins from 1.5 to 4 kHz.

    Integer-cycle bins make the signal and its slope periodic at the loop
    boundary. Random phases, amplitudes, and sparse bin selection prevent a
    stable pitch from emerging. Rotating to the quietest adjacent boundary
    further bounds the quantized PCM seam without adding an envelope (which
    would introduce low-frequency modulation).
    """
    rng = random.Random(20260716)
    duration = 0.70
    n = round(duration * SAMPLE_RATE)
    output = [0.0] * n

    first_bin = math.ceil(1500.0 * n / SAMPLE_RATE)
    last_bin = math.floor(4000.0 * n / SAMPLE_RATE)
    bins = sorted(rng.sample(range(first_bin, last_bin + 1), 320))
    for frequency_bin in bins:
        phase = rng.uniform(0.0, 2.0 * math.pi)
        # Modest amplitude variation keeps the spectrum noise-like while no
        # individual high-frequency bin dominates as a whistle.
        amplitude = rng.uniform(0.65, 1.0) / math.sqrt(len(bins))
        step = 2.0 * math.pi * frequency_bin / n
        for i in range(n):
            output[i] += amplitude * math.sin(step * i + phase)

    # Every circular edge is part of the periodic waveform. Put the smallest
    # one at the file boundary so PCM quantization leaves a tightly bounded
    # last-to-first seam.
    cut = min(range(n), key=lambda i: abs(output[(i + 1) % n] - output[i]))
    start = (cut + 1) % n
    return output[start:] + output[:start]


# ---------------------------------------------------------------------------
# Entry point.
# ---------------------------------------------------------------------------
def main() -> None:
    os.makedirs(AUDIO_DIR, exist_ok=True)

    targets = [
        ("positive.wav", synth_positive),
        ("penalty.wav", synth_penalty),
        ("tire_squeal_loop.wav", synth_tire_squeal_loop),
    ]

    print(f"Audio dir: {AUDIO_DIR}")
    for name, synth in targets:
        path = os.path.join(AUDIO_DIR, name)
        samples = synth()
        _write_wav(path, samples)
        # Report metadata for inspection. The reported peak is measured from
        # the frames actually written to disk (i.e. the *normalized* PCM), not
        # the pre-normalization synthesis peak of ``samples``.
        with wave.open(path, "rb") as w:
            n = w.getnframes()
            frames = w.readframes(n)
            pcm = struct.unpack(f"<{n}h", frames)
            peak = max((abs(v) for v in pcm), default=0) / 32767
            print(
                f"  wrote {name}: channels={w.getnchannels()} "
                f"sample_width={w.getsampwidth()}B rate={w.getframerate()} "
                f"frames={n} dur={n/SAMPLE_RATE:.3f}s "
                f"peak={peak:.3f} ({peak*100:.1f}% FS)"
            )
    print("Done. Existing six assets were not touched.")


if __name__ == "__main__":
    main()
