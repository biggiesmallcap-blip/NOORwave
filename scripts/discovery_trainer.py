#!/usr/bin/env python3
import hashlib
import json
import math
import random
import sys
from collections import Counter, defaultdict


def normalize(vec):
    norm = math.sqrt(sum(v * v for v in vec))
    if norm <= 1e-12:
        return [0.0 for _ in vec], 0.0
    return [v / norm for v in vec], norm


def cosine(a, b):
    return sum(x * y for x, y in zip(a, b))


def hashed_projection(tokens, dim):
    vec = [0.0] * dim
    for token in tokens:
        digest = hashlib.sha256(token.encode("utf-8")).digest()
        for offset in range(0, min(32, dim * 2), 2):
            bucket = digest[offset] % dim
            sign = 1.0 if digest[offset + 1] % 2 == 0 else -1.0
            vec[bucket] += sign * 0.5
    return normalize(vec)[0]


def metadata_tokens(track):
    tokens = []
    for field in ("title", "artist_name", "album_title", "best_quality", "source"):
        value = track.get(field)
        if value:
            tokens.extend(str(value).lower().replace("/", " ").replace("-", " ").split())
    duration = track.get("duration_ms")
    if duration:
        tokens.append(f"dur_{int(duration) // 30000}")
    for genre in track.get("genre_paths", []):
        tokens.extend(str(genre).lower().replace(">", " ").split())
    return [token for token in tokens if token]


def build_behavioral_embeddings(input_payload):
    dim = input_payload.get("dimension", 96)
    window = input_payload.get("window_size", 8)
    min_count = input_payload.get("min_count", 2)
    sequences = input_payload.get("sequences", [])

    counts = Counter()
    for source in sequences:
        for sequence in source.get("sequences", []):
            counts.update(sequence)

    allowed = {track_id for track_id, count in counts.items() if count >= min_count}
    co = defaultdict(lambda: defaultdict(float))
    for source in sequences:
        weight = float(source.get("weight", 1.0))
        for sequence in source.get("sequences", []):
            filtered = [track_id for track_id in sequence if track_id in allowed]
            for i, track_id in enumerate(filtered):
                left = max(0, i - window)
                right = min(len(filtered), i + window + 1)
                for j in range(left, right):
                    if i == j:
                        continue
                    other = filtered[j]
                    distance = abs(i - j)
                    co[track_id][other] += weight / max(1, distance)

    embeddings = {}
    for track_id, neighbors in co.items():
        vec = [0.0] * dim
        for other, score in neighbors.items():
            digest = hashlib.sha256(f"{track_id}:{other}".encode("utf-8")).digest()
            for offset in range(0, min(32, dim * 2), 2):
                bucket = digest[offset] % dim
                sign = 1.0 if digest[offset + 1] % 2 == 0 else -1.0
                vec[bucket] += sign * score
        embeddings[track_id] = normalize(vec)[0]
    return embeddings


def build_audio_proxy_features(tracks, dim):
    features = {}
    for track in tracks:
        clip_duration = 20000
        duration = track.get("duration_ms") or 0
        clip_start = 30000 if duration >= 90000 else max(0, (duration - clip_duration) // 2)
        vec = hashed_projection(metadata_tokens(track), dim)
        features[track["track_id"]] = {
            "vector": vec,
            "clip_start_ms": clip_start,
            "clip_duration_ms": clip_duration,
            "feature_version": "metadata-audio-proxy-v1",
        }
    return features


def fuse_embeddings(tracks, behavioral, audio, dim):
    fusion = {}
    for track in tracks:
        track_id = track["track_id"]
        b = behavioral.get(track_id)
        a = audio.get(track_id, {}).get("vector")
        playlist_memberships = track.get("playlist_memberships", 0)
        play_count = track.get("play_count", 0)
        if b and a:
            behavioral_weight, audio_weight = (
                (0.35, 0.65) if play_count < 2 and playlist_memberships == 0 else (0.7, 0.3)
            )
            vec = [
                b[i] * behavioral_weight + a[i] * audio_weight
                for i in range(dim)
            ]
            fusion[track_id] = normalize(vec)[0]
        elif b:
            fusion[track_id] = b
        elif a:
            fusion[track_id] = a
    return fusion


def similarity_neighbors(tracks, behavioral, audio, fusion, top_k):
    track_lookup = {track["track_id"]: track for track in tracks}
    items = list(fusion.items())
    neighbors = []
    for track_id, vector in items:
        scores = []
        current_track = track_lookup.get(track_id, {})
        current_artist = (current_track.get("artist_name") or "").lower()
        current_genres = set(token for token in metadata_tokens(current_track) if token.startswith("dur_") is False)
        for other_id, other_vector in items:
            if track_id == other_id:
                continue
            score = cosine(vector, other_vector)
            if score <= 0:
                continue
            behavioral_score = cosine(
                behavioral.get(track_id, [0.0] * len(vector)),
                behavioral.get(other_id, [0.0] * len(vector)),
            ) if track_id in behavioral and other_id in behavioral else 0.0
            audio_score = cosine(
                audio.get(track_id, {}).get("vector", [0.0] * len(vector)),
                audio.get(other_id, {}).get("vector", [0.0] * len(vector)),
            ) if track_id in audio and other_id in audio else 0.0

            other_track = track_lookup.get(other_id, {})
            metadata_score = 0.0
            reason_tags = []
            if current_artist and current_artist == (other_track.get("artist_name") or "").lower():
                metadata_score += 0.2
                reason_tags.append("artist_affinity")
            other_genres = set(token for token in metadata_tokens(other_track) if token.startswith("dur_") is False)
            if current_genres & other_genres:
                metadata_score += 0.18
                reason_tags.append("genre_branch")
            if current_track.get("album_title") and current_track.get("album_title") == other_track.get("album_title"):
                metadata_score += 0.12
                reason_tags.append("album_context")
            if behavioral_score > 0.35:
                reason_tags.append("behavioral")
            if audio_score > 0.35:
                reason_tags.append("audio_texture")

            total = score + metadata_score
            scores.append((other_id, total, behavioral_score, audio_score, metadata_score, sorted(set(reason_tags))))

        scores.sort(key=lambda item: item[1], reverse=True)
        for rank, (other_id, total, behavioral_score, audio_score, metadata_score, reason_tags) in enumerate(scores[:top_k], start=1):
            neighbors.append(
                {
                    "track_id": track_id,
                    "neighbor_track_id": other_id,
                    "rank": rank,
                    "score": total,
                    "behavioral_score": behavioral_score,
                    "audio_score": audio_score,
                    "metadata_score": metadata_score,
                    "reason_tags": reason_tags,
                }
            )
    return neighbors


def evaluate(neighbors, heldout_pairs):
    if not heldout_pairs:
        return {"recall_at_10": 0.0, "mrr_at_20": 0.0}

    grouped = defaultdict(list)
    for row in neighbors:
        grouped[row["track_id"]].append(row["neighbor_track_id"])

    hits = 0
    reciprocal_rank = 0.0
    for source, target in heldout_pairs:
        ranked = grouped.get(source, [])[:20]
        if target in ranked[:10]:
            hits += 1
        if target in ranked:
            reciprocal_rank += 1.0 / (ranked.index(target) + 1)

    total = len(heldout_pairs)
    return {
        "recall_at_10": hits / total if total else 0.0,
        "mrr_at_20": reciprocal_rank / total if total else 0.0,
    }


def main():
    if len(sys.argv) != 3:
        print("usage: discovery_trainer.py input.json output.json", file=sys.stderr)
        return 1

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    with open(input_path, "r", encoding="utf-8") as handle:
        payload = json.load(handle)

    random.seed(payload.get("seed", 13))
    dim = payload.get("dimension", 96)
    tracks = payload.get("tracks", [])
    behavioral = build_behavioral_embeddings(payload)
    audio = build_audio_proxy_features(tracks, dim)
    fusion = fuse_embeddings(tracks, behavioral, audio, dim)
    neighbors = similarity_neighbors(
        tracks,
        behavioral,
        audio,
        fusion,
        payload.get("top_k", 64),
    )
    metrics = evaluate(neighbors, payload.get("heldout_pairs", []))
    playable_tracks = len(tracks)
    embedded_tracks = len(fusion)
    metrics.update(
        {
            "coverage_ratio": (embedded_tracks / playable_tracks) if playable_tracks else 0.0,
            "playable_tracks": playable_tracks,
            "embedded_tracks": embedded_tracks,
            "neighbor_tracks": len({row["track_id"] for row in neighbors}),
        }
    )

    output = {
        "behavioral_embeddings": {str(key): value for key, value in behavioral.items()},
        "audio_features": {str(key): value for key, value in audio.items()},
        "fusion_embeddings": {str(key): value for key, value in fusion.items()},
        "neighbors": neighbors,
        "metrics": metrics,
    }

    with open(output_path, "w", encoding="utf-8") as handle:
        json.dump(output, handle)

    return 0


if __name__ == "__main__":
    sys.exit(main())
