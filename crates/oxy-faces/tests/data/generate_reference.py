#!/usr/bin/env python3
"""Regenerate the OpenCV reference goldens for the oxy-faces parity tests.

`oxy-faces` reimplements OpenCV's YuNet postprocessing, 5-landmark similarity
alignment, and SFace preprocessing in pure Rust. This script produces the
reference values those tests compare against, using OpenCV itself, so a change
in either implementation is caught instead of being asserted against itself.

Requirements:

    python3 -m venv /tmp/oxy-faces-ref
    /tmp/oxy-faces-ref/bin/pip install opencv-python-headless numpy
    /tmp/oxy-faces-ref/bin/python generate_reference.py

The models are read from `target/native/face-models` (`pnpm faces:prepare`) and
the portrait fixture is downloaded from its pinned commit. Both the fixture and
the goldens are committed, so the test suite needs neither OpenCV nor a network
connection at run time.
"""

from __future__ import annotations

import hashlib
import json
import sys
import urllib.request
from pathlib import Path

import cv2
import numpy as np

DATA = Path(__file__).resolve().parent
GOLDEN = DATA / "golden"
FIXTURES = DATA / "fixtures"
REPO = DATA.parents[3]
MODELS = REPO / "target" / "native" / "face-models"

# Public-domain fixture (US government work), pinned by commit. See README.md.
FIXTURE_URL = (
    "https://raw.githubusercontent.com/ageitgey/face_recognition/"
    "112d636609de01c8623366d056a67a8a1d798675/examples/two_people.jpg"
)
FIXTURE_NAME = "two_people.jpg"
FIXTURE_WIDTH = 800

DETECTOR_INPUT = 640
ALIGNED_SIZE = 112


def synthetic_rgb(width: int, height: int) -> np.ndarray:
    """Deterministic RGB pattern; `RgbImage::synthetic` mirrors this exactly."""
    x = np.arange(width, dtype=np.int64)[None, :]
    y = np.arange(height, dtype=np.int64)[:, None]
    r = (x * 3 + y * 5) % 256
    g = (x * 7 + y * 11) % 256
    b = (x * 13 + y * 17) % 256
    return np.stack([r, g, b], axis=-1).astype(np.uint8)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "oxyviewer-fixtures"})
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def ensure_fixture() -> Path:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    target = FIXTURES / FIXTURE_NAME
    if target.is_file():
        return target
    print(f"fixtures: downloading {FIXTURE_NAME}")
    source = cv2.imdecode(np.frombuffer(download(FIXTURE_URL), dtype=np.uint8), cv2.IMREAD_COLOR)
    height, width = source.shape[:2]
    scale = FIXTURE_WIDTH / width
    resized = cv2.resize(
        source, (FIXTURE_WIDTH, int(round(height * scale))), interpolation=cv2.INTER_AREA
    )
    target.write_bytes(cv2.imencode(".jpg", resized, [cv2.IMWRITE_JPEG_QUALITY, 88])[1].tobytes())
    print(f"fixtures: wrote {target} sha256={sha256_bytes(target.read_bytes())}")
    return target


def letterbox(bgr: np.ndarray) -> tuple[np.ndarray, float, float]:
    height, width = bgr.shape[:2]
    scale = min(DETECTOR_INPUT / width, DETECTOR_INPUT / height)
    new_w = max(1, min(DETECTOR_INPUT, int(round(width * scale))))
    new_h = max(1, min(DETECTOR_INPUT, int(round(height * scale))))
    resized = cv2.resize(bgr, (new_w, new_h), interpolation=cv2.INTER_LINEAR)
    canvas = np.zeros((DETECTOR_INPUT, DETECTOR_INPUT, 3), dtype=np.uint8)
    canvas[:new_h, :new_w] = resized
    return canvas, new_w / width, new_h / height


def main() -> int:
    GOLDEN.mkdir(parents=True, exist_ok=True)
    detector_path = MODELS / "face_detection_yunet_2023mar.onnx"
    embedder_path = MODELS / "face_recognition_sface_2021dec.onnx"
    if not detector_path.is_file() or not embedder_path.is_file():
        print("run `pnpm faces:prepare` first", file=sys.stderr)
        return 1

    detector = cv2.FaceDetectorYN.create(str(detector_path), "", (640, 640), 0.9, 0.3, 5000)
    recognizer = cv2.FaceRecognizerSF.create(str(embedder_path), "", 0, 0)

    # --- SFace embedding on the deterministic pattern ----------------------
    rgb = synthetic_rgb(ALIGNED_SIZE, ALIGNED_SIZE)
    feature = np.asarray(
        recognizer.feature(cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)), dtype=np.float32
    ).reshape(-1)
    write(
        GOLDEN / "sface_synthetic.json",
        {
            "input": {"width": ALIGNED_SIZE, "height": ALIGNED_SIZE, "pattern": "rgb-lcg-v1"},
            "embedding": [round(float(value), 6) for value in feature],
        },
    )

    # --- YuNet on the deterministic pattern ---------------------------------
    pattern = cv2.cvtColor(synthetic_rgb(DETECTOR_INPUT, DETECTOR_INPUT), cv2.COLOR_RGB2BGR)
    detector.setInputSize((DETECTOR_INPUT, DETECTOR_INPUT))
    _, faces = detector.detect(pattern)
    write(
        GOLDEN / "yunet_synthetic.json",
        {
            "input": {
                "width": DETECTOR_INPUT,
                "height": DETECTOR_INPUT,
                "pattern": "rgb-lcg-v1",
            },
            "faceCount": 0 if faces is None else int(faces.shape[0]),
        },
    )

    # --- Alignment on a deterministic canvas --------------------------------
    landmarks = np.array(
        [
            [100.0, 120.0],
            [180.0, 118.0],
            [140.0, 170.0],
            [104.0, 220.0],
            [176.0, 218.0],
        ],
        dtype=np.float32,
    )
    canvas = cv2.cvtColor(synthetic_rgb(320, 320), cv2.COLOR_RGB2BGR)
    row = face_row(landmarks, 0.99)
    aligned = np.asarray(recognizer.alignCrop(canvas, row))
    # The aligned crop itself is committed so the Rust warp can be compared
    # pixel by pixel instead of only through a downstream embedding.
    cv2.imwrite(str(GOLDEN / "align_synthetic_aligned.png"), aligned)
    write(
        GOLDEN / "align_synthetic.json",
        {
            "canvas": {"width": 320, "height": 320, "pattern": "rgb-lcg-v1"},
            "landmarks": landmarks.tolist(),
            "alignedSha256": sha256_bytes(np.ascontiguousarray(aligned).tobytes()),
            "alignedSum": int(aligned.astype(np.int64).sum()),
        },
    )

    # --- Full pipeline on the committed portrait fixture --------------------
    fixture = ensure_fixture()
    source = cv2.imdecode(np.frombuffer(fixture.read_bytes(), dtype=np.uint8), cv2.IMREAD_COLOR)
    height, width = source.shape[:2]
    canvas, scale_x, scale_y = letterbox(source)
    detector.setInputSize((DETECTOR_INPUT, DETECTOR_INPUT))
    _, faces = detector.detect(canvas)
    detections = []
    if faces is not None:
        for face in faces:
            # Map the letterbox detection back to source pixels; alignment then
            # runs on the full-resolution image, exactly like FaceAnalyzer.
            box = [float(face[0]) / scale_x, float(face[1]) / scale_y,
                   float(face[2]) / scale_x, float(face[3]) / scale_y]
            points = np.array(
                [[float(face[4 + index * 2]), float(face[5 + index * 2])] for index in range(5)],
                dtype=np.float32,
            )
            source_points = points / np.array([scale_x, scale_y], dtype=np.float32)
            aligned = np.asarray(recognizer.alignCrop(source, face_row(source_points, float(face[14]))))
            embedding = np.asarray(recognizer.feature(aligned), dtype=np.float32).reshape(-1)
            detections.append(
                {
                    "box": box,
                    "landmarks": source_points.tolist(),
                    "score": float(face[14]),
                    "embedding": [round(float(value), 6) for value in embedding],
                }
            )
    detections.sort(key=lambda item: item["score"], reverse=True)
    write(
        GOLDEN / "portrait_reference.json",
        {
            "fixture": FIXTURE_NAME,
            "fixtureSha256": sha256_bytes(fixture.read_bytes()),
            "sourceWidth": width,
            "sourceHeight": height,
            "detectorInputSize": DETECTOR_INPUT,
            "detectionConfidence": 0.9,
            "nmsThreshold": 0.3,
            "detections": detections,
        },
    )
    print(f"golden: portrait detections={len(detections)}")
    return 0


def face_row(landmarks: np.ndarray, score: float) -> np.ndarray:
    row = np.zeros((1, 15), dtype=np.float32)
    row[0, 4:14] = landmarks.reshape(-1)
    row[0, 14] = score
    return row


def write(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")
    print(f"golden: wrote {path}")


if __name__ == "__main__":
    raise SystemExit(main())
