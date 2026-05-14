use crate::types::{ActivationOutput, BeatEvent, ProgressEvent, ProgressSink, ProgressStage};
use crate::{CoreConfig, RhythmError};
use std::mem::size_of;

const VITERBI_BLOCK_FRAMES: usize = 256;
const DBN_PROGRESS_MIN_DELTA: f32 = 0.02;
const DBN_PROGRESS_WORK_PORTION: f32 = 0.98;

pub fn decode(
    activations: &ActivationOutput,
    config: &CoreConfig,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<BeatEvent>, RhythmError> {
    progress.on_progress(ProgressEvent {
        stage: ProgressStage::Dbn,
        progress: 0.0,
    });
    let fps = config.feature.fps;

    let mut act: Vec<[f32; 2]> = activations
        .beat
        .iter()
        .zip(activations.downbeat.iter())
        .map(|(b, d)| [*b, *d])
        .collect();

    let mut first = 0usize;
    if config.dbn.threshold > 0.0 {
        let (trimmed, offset) = threshold_activations(&act, config.dbn.threshold);
        act = trimmed;
        first = offset;
    }

    if act.is_empty() || !act.iter().any(|v| v[0] > 0.0 || v[1] > 0.0) {
        progress.on_progress(ProgressEvent {
            stage: ProgressStage::Dbn,
            progress: 1.0,
        });
        return Ok(Vec::new());
    }

    let hmms = build_hmms(config)?;
    let mut total_work = 0usize;
    for hmm in &hmms {
        total_work = total_work.saturating_add(hmm.work_units(act.len()));
    }
    let mut progress_tracker = DbnProgressTracker::new(progress, total_work.max(1));

    let mut best_logp = f64::NEG_INFINITY;
    let mut best_path: Vec<usize> = Vec::new();
    let mut best_hmm: Option<&HiddenMarkovModel> = None;

    for hmm in &hmms {
        let (path, logp) = hmm.viterbi(&act, &mut progress_tracker)?;
        if logp > best_logp {
            best_logp = logp;
            best_path = path;
            best_hmm = Some(hmm);
        }
    }

    let hmm = best_hmm.ok_or_else(|| RhythmError::Model("no HMM".to_string()))?;
    let st = &hmm.transition_model.state_space;
    let om = &hmm.observation_model;

    let mut beat_numbers = Vec::with_capacity(best_path.len());
    for &state in &best_path {
        let pos = st.state_positions[state];
        beat_numbers.push(pos.floor() as usize + 1);
    }

    let mut beat_indices = if config.dbn.correct {
        corrected_beats(&act, &best_path, om)?
    } else {
        transitions(&beat_numbers)
    };

    beat_indices.sort_unstable();
    beat_indices.dedup();

    let (refined_indices, peaks) = refine_beat_indices(&beat_indices, &act);
    let energy = frame_energy(&act);
    let confidences = normalized_peak_confidences(&energy, &peaks);

    let mut beats = Vec::with_capacity(beat_indices.len());
    let mut previous_time: Option<f32> = None;
    for (event_idx, &idx) in beat_indices.iter().enumerate() {
        let raw_time_sec = ((idx + first) as f32) / fps;
        let mut time_sec = (refined_indices[event_idx] + first as f32) / fps;
        // Keep output strictly time-ascending and deterministic.
        if let Some(prev) = previous_time {
            if time_sec <= prev {
                time_sec = raw_time_sec;
            }
            if time_sec <= prev {
                time_sec = f32::from_bits(prev.to_bits() + 1);
            }
        }
        previous_time = Some(time_sec);
        beats.push(BeatEvent {
            time_sec,
            beat_in_bar: beat_numbers[idx],
            confidence: confidences[event_idx],
        });
    }
    progress_tracker.finish();

    progress.on_progress(ProgressEvent {
        stage: ProgressStage::Dbn,
        progress: 1.0,
    });

    Ok(beats)
}

fn threshold_activations(data: &[[f32; 2]], threshold: f32) -> (Vec<[f32; 2]>, usize) {
    let mut first = 0usize;
    let mut last = 0usize;
    let mut found = false;
    for (i, v) in data.iter().enumerate() {
        if v[0] >= threshold || v[1] >= threshold {
            if !found {
                first = i;
                found = true;
            }
            last = i + 1;
        }
    }
    if !found {
        return (Vec::new(), 0);
    }
    (data[first..last].to_vec(), first)
}

fn frame_energy(activations: &[[f32; 2]]) -> Vec<f32> {
    activations.iter().map(|v| v[0] + v[1]).collect()
}

fn refine_beat_indices(indices: &[usize], activations: &[[f32; 2]]) -> (Vec<f32>, Vec<usize>) {
    let energy = frame_energy(activations);
    let count = energy.len();
    let mut refined = Vec::with_capacity(indices.len());
    let mut peaks = Vec::with_capacity(indices.len());

    for &idx in indices {
        if idx == 0 || idx + 1 >= count {
            refined.push(idx as f32);
            peaks.push(idx);
            continue;
        }

        let left = idx.saturating_sub(1);
        let right = (idx + 1).min(count - 1);
        let mut peak = left;
        let mut best = f32::NEG_INFINITY;
        for (offset, value) in energy[left..=right].iter().enumerate() {
            if *value > best {
                best = *value;
                peak = left + offset;
            }
        }
        peaks.push(peak);

        if peak == 0 || peak + 1 >= count {
            refined.push(peak as f32);
            continue;
        }

        let y1 = energy[peak - 1] as f64;
        let y2 = energy[peak] as f64;
        let y3 = energy[peak + 1] as f64;
        let denom = y1 - 2.0 * y2 + y3;
        if denom.abs() < 1e-12 {
            refined.push(peak as f32);
            continue;
        }
        let delta = (0.5 * (y1 - y3) / denom).clamp(-0.5, 0.5);
        refined.push(peak as f32 + delta as f32);
    }

    (refined, peaks)
}

fn normalized_peak_confidences(energy: &[f32], peaks: &[usize]) -> Vec<f32> {
    if energy.is_empty() {
        return vec![0.5; peaks.len()];
    }
    let min_e = energy.iter().copied().fold(f32::INFINITY, f32::min);
    let max_e = energy.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if (max_e - min_e).abs() < 1e-6 {
        return vec![0.5; peaks.len()];
    }
    peaks
        .iter()
        .map(|&peak| ((energy[peak] - min_e) / (max_e - min_e)).clamp(0.0, 1.0))
        .collect()
}

fn transitions(beat_numbers: &[usize]) -> Vec<usize> {
    let mut idx = Vec::new();
    for i in 1..beat_numbers.len() {
        if beat_numbers[i] != beat_numbers[i - 1] {
            idx.push(i);
        }
    }
    idx
}

fn corrected_beats(
    activations: &[[f32; 2]],
    path: &[usize],
    om: &ObservationModel,
) -> Result<Vec<usize>, RhythmError> {
    let mut beat_range = Vec::with_capacity(path.len());
    for &state in path {
        beat_range.push(om.pointers[state] >= 1);
    }
    if !beat_range.iter().any(|v| *v) {
        return Ok(Vec::new());
    }
    let mut idx = Vec::new();
    for i in 1..beat_range.len() {
        if beat_range[i] != beat_range[i - 1] {
            idx.push(i);
        }
    }
    if beat_range[0] {
        idx.insert(0, 0);
    }
    if *beat_range.last().unwrap() {
        idx.push(beat_range.len());
    }
    let mut beats = Vec::new();
    for pair in idx.chunks(2) {
        if pair.len() != 2 {
            continue;
        }
        let left = pair[0];
        let right = pair[1];
        let mut best_idx = left;
        let mut best_val = f32::MIN;
        for (i, act) in activations.iter().enumerate().take(right).skip(left) {
            let v = act[0].max(act[1]);
            if v > best_val {
                best_val = v;
                best_idx = i;
            }
        }
        beats.push(best_idx);
    }
    Ok(beats)
}

fn build_hmms(config: &CoreConfig) -> Result<Vec<HiddenMarkovModel>, RhythmError> {
    let beats_per_bar = &config.dbn.beats_per_bar;
    let min_bpm = config.dbn.min_bpm;
    let max_bpm = config.dbn.max_bpm;
    let num_tempi = config.dbn.num_tempi;
    let transition_lambda = config.dbn.transition_lambda;

    let min_interval = 60.0 * config.feature.fps / max_bpm;
    let max_interval = 60.0 * config.feature.fps / min_bpm;

    let mut hmms = Vec::new();
    for &beats in beats_per_bar {
        let st = BarStateSpace::new(beats, min_interval, max_interval, Some(num_tempi))?;
        let tm = BarTransitionModel::new(&st, vec![Some(transition_lambda); st.num_beats])?;
        let om = ObservationModel::new(&st, config.dbn.observation_lambda)?;
        hmms.push(HiddenMarkovModel::new(tm, om));
    }
    Ok(hmms)
}

struct BeatStateSpace {
    num_states: usize,
    state_positions: Vec<f32>,
    state_intervals: Vec<usize>,
    first_states: Vec<usize>,
    last_states: Vec<usize>,
}

impl BeatStateSpace {
    fn new(
        min_interval: f32,
        max_interval: f32,
        num_intervals: Option<usize>,
    ) -> Result<Self, RhythmError> {
        let mut intervals: Vec<usize> =
            ((min_interval.round() as usize)..=(max_interval.round() as usize)).collect();
        if let Some(target) = num_intervals {
            if target < intervals.len() {
                let mut num_log = target;
                loop {
                    let mut candidate = logspace_intervals(min_interval, max_interval, num_log);
                    candidate.sort();
                    candidate.dedup();
                    if candidate.len() >= target {
                        intervals = candidate;
                        break;
                    }
                    num_log += 1;
                }
            }
        }
        let num_states: usize = intervals.iter().sum();
        let mut first_states = Vec::with_capacity(intervals.len());
        let mut last_states = Vec::with_capacity(intervals.len());
        let mut state_positions = vec![0.0; num_states];
        let mut state_intervals = vec![0usize; num_states];
        let mut idx = 0usize;
        for &interval in &intervals {
            first_states.push(idx);
            last_states.push(idx + interval - 1);
            for i in 0..interval {
                state_positions[idx + i] = (i as f32) / (interval as f32);
                state_intervals[idx + i] = interval;
            }
            idx += interval;
        }
        Ok(Self {
            num_states,
            state_positions,
            state_intervals,
            first_states,
            last_states,
        })
    }
}

#[derive(Clone)]
struct BarStateSpace {
    num_beats: usize,
    num_states: usize,
    state_positions: Vec<f32>,
    state_intervals: Vec<usize>,
    first_states: Vec<Vec<usize>>,
    last_states: Vec<Vec<usize>>,
}

impl BarStateSpace {
    fn new(
        num_beats: usize,
        min_interval: f32,
        max_interval: f32,
        num_intervals: Option<usize>,
    ) -> Result<Self, RhythmError> {
        let bss = BeatStateSpace::new(min_interval, max_interval, num_intervals)?;
        let mut state_positions = Vec::new();
        let mut state_intervals = Vec::new();
        let mut first_states = Vec::new();
        let mut last_states = Vec::new();
        let mut offset = 0usize;
        for beat in 0..num_beats {
            for &pos in &bss.state_positions {
                state_positions.push(pos + beat as f32);
            }
            state_intervals.extend_from_slice(&bss.state_intervals);
            first_states.push(bss.first_states.iter().map(|v| v + offset).collect());
            last_states.push(bss.last_states.iter().map(|v| v + offset).collect());
            offset += bss.num_states;
        }
        Ok(Self {
            num_beats,
            num_states: offset,
            state_positions,
            state_intervals,
            first_states,
            last_states,
        })
    }
}

#[derive(Clone)]
struct ObservationModel {
    pointers: Vec<usize>,
    observation_lambda: usize,
}

impl ObservationModel {
    fn new(state_space: &BarStateSpace, observation_lambda: usize) -> Result<Self, RhythmError> {
        let border = 1.0 / observation_lambda as f32;
        let mut pointers = vec![0usize; state_space.num_states];
        for (i, pos) in state_space.state_positions.iter().enumerate() {
            let beat_pos = pos % 1.0;
            if beat_pos < border {
                pointers[i] = 1;
            }
            if *pos < border {
                pointers[i] = 2;
            }
        }
        Ok(Self {
            pointers,
            observation_lambda,
        })
    }

    fn log_densities(&self, observations: &[[f32; 2]]) -> Vec<[f64; 3]> {
        let mut out = Vec::with_capacity(observations.len());
        for obs in observations {
            let beat = obs[0] as f64;
            let downbeat = obs[1] as f64;
            let sum = beat + downbeat;
            let none = (1.0 - sum) / (self.observation_lambda as f64 - 1.0);
            out.push([
                log_or_neg_inf(none),
                log_or_neg_inf(beat),
                log_or_neg_inf(downbeat),
            ]);
        }
        out
    }
}

#[derive(Clone)]
struct TransitionModel {
    states: Vec<usize>,
    pointers: Vec<usize>,
    log_probabilities: Vec<f64>,
    state_space: BarStateSpace,
}

struct BarTransitionModel {
    tm: TransitionModel,
}

impl BarTransitionModel {
    fn new(
        state_space: &BarStateSpace,
        transition_lambda: Vec<Option<f32>>,
    ) -> Result<Self, RhythmError> {
        if transition_lambda.len() != state_space.num_beats {
            return Err(RhythmError::InvalidInput(
                "transition_lambda length mismatch".to_string(),
            ));
        }
        let mut states = Vec::new();
        let mut prev_states = Vec::new();
        let mut probabilities = Vec::new();

        for state in 0..state_space.num_states {
            let is_first = state_space.first_states.iter().any(|v| v.contains(&state));
            if !is_first {
                states.push(state);
                prev_states.push(state - 1);
                probabilities.push(1.0);
            }
        }

        for (beat, lambda) in transition_lambda
            .iter()
            .enumerate()
            .take(state_space.num_beats)
        {
            let to_states = &state_space.first_states[beat];
            let from_states = &state_space.last_states
                [(beat + state_space.num_beats - 1) % state_space.num_beats];
            let from_int: Vec<usize> = from_states
                .iter()
                .map(|s| state_space.state_intervals[*s])
                .collect();
            let to_int: Vec<usize> = to_states
                .iter()
                .map(|s| state_space.state_intervals[*s])
                .collect();
            let prob = exponential_transition(&from_int, &to_int, *lambda);
            for (i_from, row) in prob.iter().enumerate() {
                for (i_to, p) in row.iter().enumerate() {
                    if *p > 0.0 {
                        states.push(to_states[i_to]);
                        prev_states.push(from_states[i_from]);
                        probabilities.push(*p);
                    }
                }
            }
        }

        let (states, pointers, log_probabilities) =
            make_sparse(&states, &prev_states, &probabilities)?;

        Ok(Self {
            tm: TransitionModel {
                states,
                pointers,
                log_probabilities,
                state_space: state_space.clone(),
            },
        })
    }
}

#[derive(Clone)]
struct HiddenMarkovModel {
    transition_model: TransitionModel,
    observation_model: ObservationModel,
    initial_distribution: Vec<f64>,
}

impl HiddenMarkovModel {
    fn new(transition_model: BarTransitionModel, observation_model: ObservationModel) -> Self {
        let num_states = transition_model.tm.pointers.len() - 1;
        let init = vec![1.0 / num_states as f64; num_states];
        Self {
            transition_model: transition_model.tm,
            observation_model,
            initial_distribution: init,
        }
    }

    fn work_units(&self, num_frames: usize) -> usize {
        let num_states = self.transition_model.pointers.len().saturating_sub(1);
        num_frames.saturating_mul(num_states).saturating_mul(2)
    }

    fn viterbi(
        &self,
        observations: &[[f32; 2]],
        progress: &mut DbnProgressTracker<'_>,
    ) -> Result<(Vec<usize>, f64), RhythmError> {
        let num_states = self.transition_model.pointers.len() - 1;
        let num_frames = observations.len();
        if num_frames == 0 {
            return Ok((Vec::new(), f64::NEG_INFINITY));
        }
        let log_densities = self.observation_model.log_densities(observations);

        let mut previous: Vec<f64> = self.initial_distribution.iter().map(|v| v.ln()).collect();
        let mut current = vec![f64::NEG_INFINITY; num_states];

        let num_blocks = num_frames.div_ceil(VITERBI_BLOCK_FRAMES);
        let mut boundary_scores = Vec::with_capacity(num_blocks);
        for block_idx in 0..num_blocks {
            boundary_scores.push(previous.clone());
            let start = block_idx * VITERBI_BLOCK_FRAMES;
            let end = (start + VITERBI_BLOCK_FRAMES).min(num_frames);
            for dens in &log_densities[start..end] {
                self.run_viterbi_frame(&previous, &mut current, dens, None);
                previous.clone_from_slice(&current);
            }
            progress.advance((end - start).saturating_mul(num_states));
        }

        let mut best_state = 0usize;
        let mut best_log = f64::NEG_INFINITY;
        for (i, v) in previous.iter().enumerate() {
            if *v > best_log {
                best_log = *v;
                best_state = i;
            }
        }
        if best_log.is_infinite() && best_log.is_sign_negative() {
            // Keep global progress consistent with expected per-HMM work.
            progress.advance(num_frames.saturating_mul(num_states));
            return Ok((Vec::new(), best_log));
        }

        let mut path = vec![0usize; num_frames];
        let mut end_state = best_state;

        for block_idx in (0..num_blocks).rev() {
            let start = block_idx * VITERBI_BLOCK_FRAMES;
            let end = (start + VITERBI_BLOCK_FRAMES).min(num_frames);
            let block_len = end - start;
            let mut local_prev = boundary_scores[block_idx].clone();
            let mut local_curr = vec![f64::NEG_INFINITY; num_states];
            let mut bt = allocate_backpointers(num_states, block_len)?;

            for (local_frame, dens) in log_densities[start..end].iter().enumerate() {
                let row_start = local_frame * num_states;
                let row_end = row_start + num_states;
                self.run_viterbi_frame(
                    &local_prev,
                    &mut local_curr,
                    dens,
                    Some(&mut bt[row_start..row_end]),
                );
                local_prev.clone_from_slice(&local_curr);
            }
            progress.advance(block_len.saturating_mul(num_states));

            let mut state = end_state;
            for local_frame in (0..block_len).rev() {
                path[start + local_frame] = state;
                state = bt[local_frame * num_states + state] as usize;
            }
            end_state = state;
        }
        Ok((path, best_log))
    }

    fn run_viterbi_frame(
        &self,
        previous: &[f64],
        current: &mut [f64],
        dens: &[f64; 3],
        mut backpointers: Option<&mut [u32]>,
    ) {
        for state in 0..current.len() {
            let density = dens[self.observation_model.pointers[state]];
            let mut best = f64::NEG_INFINITY;
            let mut best_prev = 0usize;
            let start = self.transition_model.pointers[state];
            let end = self.transition_model.pointers[state + 1];
            for idx in start..end {
                let prev_state = self.transition_model.states[idx];
                let trans = self.transition_model.log_probabilities[idx];
                let score = previous[prev_state] + trans + density;
                if score > best {
                    best = score;
                    best_prev = prev_state;
                }
            }
            current[state] = best;
            if let Some(bt_row) = backpointers.as_deref_mut() {
                bt_row[state] = best_prev as u32;
            }
        }
    }
}

struct DbnProgressTracker<'a> {
    sink: &'a mut dyn ProgressSink,
    total_work: f64,
    completed_work: f64,
    last_progress: f32,
}

impl<'a> DbnProgressTracker<'a> {
    fn new(sink: &'a mut dyn ProgressSink, total_work: usize) -> Self {
        Self {
            sink,
            total_work: total_work as f64,
            completed_work: 0.0,
            last_progress: 0.0,
        }
    }

    fn advance(&mut self, units: usize) {
        if units == 0 {
            return;
        }
        self.completed_work = (self.completed_work + units as f64).min(self.total_work);
        self.maybe_emit(false);
    }

    fn finish(&mut self) {
        self.completed_work = self.total_work;
        self.maybe_emit(true);
    }

    fn maybe_emit(&mut self, force: bool) {
        if self.total_work <= 0.0 {
            return;
        }
        let work_ratio = (self.completed_work / self.total_work).clamp(0.0, 1.0);
        let progress = (work_ratio as f32 * DBN_PROGRESS_WORK_PORTION).clamp(0.0, 1.0);
        let progressed_enough = (progress - self.last_progress) >= DBN_PROGRESS_MIN_DELTA;
        if force || progressed_enough {
            self.sink.on_progress(ProgressEvent {
                stage: ProgressStage::Dbn,
                progress,
            });
            self.last_progress = progress;
        }
    }
}

fn allocate_backpointers(num_states: usize, num_frames: usize) -> Result<Vec<u32>, RhythmError> {
    if num_states > u32::MAX as usize {
        return Err(RhythmError::Model(format!(
            "decode state count {} exceeds u32 backpointer capacity",
            num_states
        )));
    }
    let entries = num_states.checked_mul(num_frames).ok_or_else(|| {
        RhythmError::Model("decode backpointer allocation size overflowed".to_string())
    })?;
    let mut bt = Vec::<u32>::new();
    if bt.try_reserve_exact(entries).is_err() {
        let bytes = entries.saturating_mul(size_of::<u32>());
        return Err(RhythmError::Model(format!(
            "decode backpointer allocation exceeded memory (states={}, block_frames={}, entries={}, bytes={})",
            num_states, num_frames, entries, bytes
        )));
    }
    bt.resize(entries, 0u32);
    Ok(bt)
}

fn exponential_transition(
    from_intervals: &[usize],
    to_intervals: &[usize],
    transition_lambda: Option<f32>,
) -> Vec<Vec<f32>> {
    if transition_lambda.is_none() {
        let mut prob = vec![vec![0.0; to_intervals.len()]; from_intervals.len()];
        for (i, from) in from_intervals.iter().enumerate() {
            for (j, to) in to_intervals.iter().enumerate() {
                if from == to {
                    prob[i][j] = 1.0;
                }
            }
        }
        return prob;
    }
    let lambda = transition_lambda.unwrap();
    let mut prob = vec![vec![0.0; to_intervals.len()]; from_intervals.len()];
    for (i, from) in from_intervals.iter().enumerate() {
        let mut sum = 0.0f32;
        for (j, to) in to_intervals.iter().enumerate() {
            let ratio = (*to as f32) / (*from as f32);
            let v = (-lambda * (ratio - 1.0).abs()).exp();
            prob[i][j] = v;
            sum += v;
        }
        if sum > 0.0 {
            for p in prob[i].iter_mut() {
                *p /= sum;
            }
        }
    }
    prob
}

type SparseTransition = (Vec<usize>, Vec<usize>, Vec<f64>);

fn make_sparse(
    states: &[usize],
    prev_states: &[usize],
    probabilities: &[f32],
) -> Result<SparseTransition, RhythmError> {
    let num_states = prev_states.iter().copied().max().unwrap_or(0) + 1;
    let mut sums = vec![0.0f32; num_states];
    for (&p, &prob) in prev_states.iter().zip(probabilities.iter()) {
        sums[p] += prob;
    }
    for s in sums {
        if (s - 1.0).abs() > 1e-3 {
            return Err(RhythmError::Model(
                "transition probabilities do not sum to 1".to_string(),
            ));
        }
    }

    let mut per_state: Vec<Vec<(usize, f64)>> = vec![Vec::new(); num_states];
    for ((&state, &prev), &prob) in states
        .iter()
        .zip(prev_states.iter())
        .zip(probabilities.iter())
    {
        per_state[state].push((prev, prob as f64));
    }
    let mut out_states = Vec::new();
    let mut pointers = Vec::with_capacity(num_states + 1);
    let mut log_probs = Vec::new();
    let mut offset = 0usize;
    pointers.push(0);
    for list in per_state {
        for (prev, prob) in list {
            out_states.push(prev);
            log_probs.push(log_or_neg_inf(prob));
            offset += 1;
        }
        pointers.push(offset);
    }
    Ok((out_states, pointers, log_probs))
}

fn log_or_neg_inf(v: f64) -> f64 {
    if v <= 0.0 {
        f64::NEG_INFINITY
    } else {
        v.ln()
    }
}

fn logspace_intervals(min_interval: f32, max_interval: f32, count: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(count);
    let min = min_interval as f64;
    let max = max_interval as f64;
    let log_min = min.log2();
    let log_max = max.log2();
    if count == 1 {
        out.push(min_interval.round() as usize);
        return out;
    }
    for i in 0..count {
        let t = i as f64 / (count as f64 - 1.0);
        let v = 2f64.powf(log_min + t * (log_max - log_min));
        out.push(v.round() as usize);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::decode;
    use crate::types::{ActivationOutput, ProgressEvent, ProgressSink, ProgressStage};
    use crate::CoreConfig;

    struct Recorder {
        events: Vec<ProgressEvent>,
    }

    impl ProgressSink for Recorder {
        fn on_progress(&mut self, event: ProgressEvent) {
            self.events.push(event);
        }
    }

    #[test]
    fn decode_emits_intermediate_dbn_progress() {
        let mut beat = vec![0.01f32; 800];
        let mut downbeat = vec![0.005f32; 800];
        for i in (0..800).step_by(50) {
            beat[i] = 0.35;
            if i % 200 == 0 {
                downbeat[i] = 0.25;
            }
        }
        let activations = ActivationOutput { beat, downbeat };
        let config = CoreConfig::default();
        let mut sink = Recorder { events: Vec::new() };

        let _beats = decode(&activations, &config, &mut sink).expect("decode should succeed");
        let dbn_events: Vec<ProgressEvent> = sink
            .events
            .iter()
            .copied()
            .filter(|event| event.stage == ProgressStage::Dbn)
            .collect();

        assert!(
            dbn_events.len() > 2,
            "expected DBN progress with intermediate ticks"
        );
        assert!(
            (dbn_events.first().unwrap().progress - 0.0).abs() < f32::EPSILON,
            "first DBN progress should be 0.0"
        );
        assert!(
            (dbn_events.last().unwrap().progress - 1.0).abs() < f32::EPSILON,
            "final DBN progress should be 1.0"
        );
        for pair in dbn_events.windows(2) {
            assert!(
                pair[1].progress >= pair[0].progress,
                "DBN progress should be monotonic"
            );
        }
    }
}
