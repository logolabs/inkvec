"""A selective scan that needs no CUDA kernel, so MambaIRv2 can run anywhere.

`mamba-ssm` ships its scan as a compiled CUDA extension and publishes no Windows
wheel, which is enough to make the upscaler unimportable on a machine that has a
perfectly good GPU. It is not needed. The scan is one first-order linear
recurrence per (batch, channel, state) triple:

    x_i = a_i * x_{i-1} + b_i,        y_i = <x_i, C_i> + D * u_i

which is associative -- the pair (a, b) composes as

    (a1, b1) . (a2, b2) = (a1 a2,  a2 b1 + b2)

-- and therefore parallel. A Hillis-Steele doubling pass computes the whole
prefix in log2(L) steps instead of L. This is also the numerically safe
formulation: the a-products are exp(negative) and stay inside (0, 1], where the
closed form `exp(S_i) * sum_k b_k exp(-S_k)` overflows on `exp(-S_k)`.

The recurrence runs in chunks along the sequence so the (batch, dim, chunk,
state) intermediates stay small. Only the carried state crosses a chunk
boundary, so the result is exact rather than approximate.

`install()` binds this in place of the real package. If `mamba_ssm` is genuinely
installed it is left alone, since its CUDA kernel is faster than this.

Verified against a plain Python loop by `python -m inkvec_sr.scan`.
"""
from __future__ import annotations

import sys
import types

import torch
import torch.nn.functional as F

#: Sequence chunk. Larger is fewer Python steps and more peak memory.
CHUNK = 256


def _scan_chunk(a: torch.Tensor, b: torch.Tensor,
                carry: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
    """Inclusive scan of x_i = a_i x_{i-1} + b_i along dim -2, starting at `carry`.

    a, b: (..., L, N).  carry: (..., N).  Returns (x, last_state).
    """
    b = b.clone()
    b[..., 0, :] = b[..., 0, :] + a[..., 0, :] * carry
    a = a.clone()
    L = a.shape[-2]
    d = 1
    while d < L:
        # Each element currently holds the recurrence run over the last d steps;
        # composing it with the one d places back doubles that reach.
        a_lo, b_lo = a[..., :-d, :], b[..., :-d, :]
        a_hi = a[..., d:, :]
        b[..., d:, :] = b[..., d:, :] + a_hi * b_lo
        a[..., d:, :] = a_hi * a_lo
        d *= 2
    return b, b[..., -1, :]


def selective_scan_fn(u, delta, A, B, C, D=None, z=None, delta_bias=None,
                      delta_softplus=False, return_last_state=False):
    """Drop-in for `mamba_ssm.ops.selective_scan_interface.selective_scan_fn`.

    Covers the shapes MambaIRv2 uses: u, delta (b, d, L); A (d, N);
    B, C (b, g, N, L) with d a multiple of g; D (d,); delta_bias (d,).
    """
    dtype_in = u.dtype
    u = u.float()
    delta = delta.float()
    if delta_bias is not None:
        delta = delta + delta_bias[..., None].float()
    if delta_softplus:
        delta = F.softplus(delta)

    bsz, dim, L = u.shape
    N = A.shape[1]

    def expand(t: torch.Tensor) -> torch.Tensor:
        # (b, g, N, L) -> (b, d, L, N), each group repeated d/g times, matching
        # the flattening `xs.view(b, -1, L)` of an original (b, g, d/g, L).
        g = t.shape[1]
        t = t.float().unsqueeze(2).expand(bsz, g, dim // g, N, L)
        return t.reshape(bsz, dim, N, L).permute(0, 1, 3, 2)

    Bx = expand(B)
    Cx = expand(C)
    A = A.float()

    x = A.new_zeros((bsz, dim, N))
    ys = []
    for s in range(0, L, CHUNK):
        e = min(s + CHUNK, L)
        d_ = delta[:, :, s:e]                                        # (b, d, c)
        a = torch.exp(d_.unsqueeze(-1) * A[None, :, None, :])        # (b, d, c, N)
        b = d_.unsqueeze(-1) * Bx[:, :, s:e] * u[:, :, s:e].unsqueeze(-1)
        xs, x = _scan_chunk(a, b, x)
        ys.append((xs * Cx[:, :, s:e]).sum(-1))                      # (b, d, c)

    y = torch.cat(ys, dim=-1)
    if D is not None:
        y = y + u * D.float()[:, None]
    if z is not None:
        y = y * F.silu(z.float())
    out = y.to(dtype_in)
    return (out, x) if return_last_state else out


def install() -> str:
    """Make `from mamba_ssm.ops.selective_scan_interface import ...` resolve.

    Returns which implementation will be used, for the caller to report.
    """
    if "mamba_ssm.ops.selective_scan_interface" in sys.modules:
        return "already bound"
    try:  # the real thing, if it is installed: its CUDA kernel is faster
        import importlib
        import importlib.util

        spec = importlib.util.find_spec("mamba_ssm")
        if spec is not None and spec.submodule_search_locations:
            # Bind the package by hand. Recent mamba-ssm initialisers eagerly
            # import language-model code that breaks against unrelated
            # Transformers builds, and MambaIRv2 needs only this one submodule.
            package = types.ModuleType("mamba_ssm")
            package.__path__ = list(spec.submodule_search_locations)
            package.__package__ = "mamba_ssm"
            package.__spec__ = spec
            sys.modules["mamba_ssm"] = package
            importlib.import_module("mamba_ssm.ops.selective_scan_interface")
            return "mamba-ssm CUDA kernel"
    except Exception:  # noqa: BLE001 -- any failure means fall back to ours
        sys.modules.pop("mamba_ssm", None)

    root = types.ModuleType("mamba_ssm")
    ops = types.ModuleType("mamba_ssm.ops")
    iface = types.ModuleType("mamba_ssm.ops.selective_scan_interface")
    iface.selective_scan_fn = selective_scan_fn
    iface.selective_scan_ref = selective_scan_fn
    ops.selective_scan_interface = iface
    root.ops = ops
    sys.modules.update({"mamba_ssm": root, "mamba_ssm.ops": ops,
                        "mamba_ssm.ops.selective_scan_interface": iface})
    return "vendored parallel scan"


def _selfcheck() -> None:
    torch.manual_seed(0)
    bsz, g, h, N, L = 2, 4, 3, 5, 300
    dim = g * h
    u = torch.randn(bsz, dim, L)
    delta = torch.randn(bsz, dim, L)
    A = -torch.rand(dim, N) - 0.1
    B = torch.randn(bsz, g, N, L)
    C = torch.randn(bsz, g, N, L)
    D = torch.randn(dim)
    bias = torch.randn(dim)

    fast = selective_scan_fn(u, delta, A, B, C, D, delta_bias=bias,
                             delta_softplus=True)

    # The definition, written out one step at a time.
    d = F.softplus(delta + bias[..., None])
    Be = B.unsqueeze(2).expand(bsz, g, h, N, L).reshape(bsz, dim, N, L)
    Ce = C.unsqueeze(2).expand(bsz, g, h, N, L).reshape(bsz, dim, N, L)
    x = torch.zeros(bsz, dim, N)
    ys = []
    for i in range(L):
        a = torch.exp(d[:, :, i, None] * A[None])
        x = a * x + d[:, :, i, None] * Be[:, :, :, i] * u[:, :, i, None]
        ys.append((x * Ce[:, :, :, i]).sum(-1))
    slow = torch.stack(ys, dim=2) + u * D[:, None]

    err = (fast - slow).abs().max().item()
    rel = err / slow.abs().max().item()
    print(f"max abs error vs the sequential definition: {err:.3e}  "
          f"(relative {rel:.3e})")
    assert rel < 1e-4, "scan does not reproduce the recurrence"
    print("OK")


if __name__ == "__main__":
    _selfcheck()
