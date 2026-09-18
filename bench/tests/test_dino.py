"""Tests for DinoScore.

These pin the *properties* the metric must have — identity scores 1, dissimilar
content scores lower, the ordering tracks visual similarity — and deliberately assert
no absolute thresholds beyond those, because every number here moves with the
checkpoint. A test that hard-codes "0.94" would be testing the weights, not the code.
"""
from __future__ import annotations

import numpy as np
import pytest

from inkvec_bench import render
from inkvec_bench.metrics import dino as m_dino

pytestmark = pytest.mark.skipif(
    not m_dino.available(),
    reason=f"DINO backbone unavailable: {m_dino.load_error() or 'unknown'}",
)


def _svg(body: str) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" '
        f'width="100" height="100">{body}</svg>'
    )


RED_CIRCLE = _svg('<circle cx="50" cy="50" r="40" fill="#e33"/>')
RED_CIRCLE_NUDGED = _svg('<circle cx="51" cy="50" r="40" fill="#e33"/>')
BLUE_TEXT_BARS = _svg(
    '<rect x="5" y="10" width="90" height="12" fill="#036"/>'
    '<rect x="5" y="34" width="60" height="12" fill="#036"/>'
    '<rect x="5" y="58" width="80" height="12" fill="#036"/>'
    '<rect x="5" y="82" width="40" height="12" fill="#036"/>'
)


def _rgb(svg: str, px: int = 128) -> np.ndarray:
    return render.composite(render.render(svg, px, px))


class TestDinoScore:
    def test_identical_images_score_one(self):
        a = _rgb(RED_CIRCLE)
        assert m_dino.dino_score(a, a) == pytest.approx(1.0, abs=1e-4)

    def test_different_content_scores_lower_than_identity(self):
        circle = _rgb(RED_CIRCLE)
        bars = _rgb(BLUE_TEXT_BARS)
        cross = m_dino.dino_score(circle, bars)
        assert cross < m_dino.dino_score(circle, circle)
        # A cosine similarity, so it cannot leave [-1, 1] whatever the inputs.
        assert -1.0 <= cross <= 1.0

    def test_near_identical_beats_unrelated(self):
        """Ordering is the property that makes the metric useful for ranking tracers."""
        ref = _rgb(RED_CIRCLE)
        near = m_dino.dino_score(ref, _rgb(RED_CIRCLE_NUDGED))
        far = m_dino.dino_score(ref, _rgb(BLUE_TEXT_BARS))
        assert near > far

    def test_symmetric(self):
        a, b = _rgb(RED_CIRCLE), _rgb(BLUE_TEXT_BARS)
        assert m_dino.dino_score(a, b) == pytest.approx(m_dino.dino_score(b, a), abs=1e-5)

    def test_batch_embed_matches_pairwise_score(self):
        a, b = _rgb(RED_CIRCLE), _rgb(BLUE_TEXT_BARS)
        feats = m_dino.embed([a, b])
        assert feats.shape[0] == 2
        # Rows are L2-normalised, so the dot product must reproduce the score exactly.
        assert np.linalg.norm(feats[0]) == pytest.approx(1.0, abs=1e-5)
        assert float(feats[0] @ feats[1]) == pytest.approx(m_dino.dino_score(a, b), abs=1e-5)


class TestBackboneSelection:
    def test_default_backbone_is_a_known_one(self):
        assert m_dino.DEFAULT_BACKBONE in m_dino.BACKBONES

    def test_unknown_backbone_raises(self):
        with pytest.raises(ValueError):
            m_dino.dino_score(_rgb(RED_CIRCLE), _rgb(RED_CIRCLE), backbone="dinov9")

    @pytest.mark.skipif(not m_dino.available("dinov2"), reason="dinov2 weights unavailable")
    def test_dinov2_backbone_also_scores_identity_one(self):
        """The calibration backbone must behave the same way, or it cannot calibrate."""
        a = _rgb(RED_CIRCLE)
        assert m_dino.dino_score(a, a, backbone="dinov2") == pytest.approx(1.0, abs=1e-4)
        assert m_dino.dino_score(a, _rgb(BLUE_TEXT_BARS), backbone="dinov2") < 1.0


class TestDegradation:
    def test_wiring_into_raster_compare(self):
        from inkvec_bench.metrics import raster as m_raster

        m = m_raster.compare(_rgb(RED_CIRCLE), _rgb(RED_CIRCLE))
        assert "dino" in m
        assert m["dino"] == pytest.approx(1.0, abs=1e-4)
        assert m_raster.perceptual_available()["dino"] is True

    def test_unavailable_backbone_yields_nan_not_an_exception(self, monkeypatch):
        """A missing checkpoint must cost one column, not the whole run."""
        monkeypatch.setattr(m_dino, "_model", lambda backbone: None)
        a = _rgb(RED_CIRCLE)
        assert np.isnan(m_dino.dino_score(a, a))
        assert m_dino.embed([a]) is None
