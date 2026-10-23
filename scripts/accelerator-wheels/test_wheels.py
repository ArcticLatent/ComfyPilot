"""Smoke-tests built add-on wheels on the local GPU.

Install the wheels into a venv with the matching torch, then run:

    python scripts/accelerator-wheels/test_wheels.py [sage2 sage3 flash nunchaku]

Each attention kernel is compared with torch's scaled_dot_product_attention on
random inputs; quantized kernels (SageAttention) are expected to be close, not
exact. Nunchaku is checked by loading its compiled extension.
"""

import sys
import time

import torch
import torch.nn.functional as F

DEVICE = "cuda"
DTYPE = torch.bfloat16
# batch, heads, sequence length, head dim: a mid-size attention call.
B, H, S, D = 1, 16, 4096, 128


def reference(q, k, v):
    return F.scaled_dot_product_attention(q, k, v)


def compare(name, out, ref, min_cosine):
    cosine = F.cosine_similarity(out.float().flatten(), ref.float().flatten(), dim=0).item()
    max_err = (out.float() - ref.float()).abs().max().item()
    ok = cosine >= min_cosine and torch.isfinite(out).all().item()
    print(f"{'PASS' if ok else 'FAIL'} {name}: cosine={cosine:.5f} max_abs_err={max_err:.4f}")
    return ok


def timed(fn):
    torch.cuda.synchronize()
    start = time.perf_counter()
    out = fn()
    torch.cuda.synchronize()
    return out, (time.perf_counter() - start) * 1000


def inputs():
    torch.manual_seed(0)
    return [torch.randn(B, H, S, D, device=DEVICE, dtype=DTYPE) for _ in range(3)]


def test_sage2():
    from sageattention import sageattn

    q, k, v = inputs()
    ref = reference(q, k, v)
    sageattn(q, k, v, tensor_layout="HND")  # warm-up / JIT
    out, ms = timed(lambda: sageattn(q, k, v, tensor_layout="HND", is_causal=False))
    print(f"     sageattn {ms:.2f} ms")
    return compare("sageattention 2", out, ref, 0.99)


def test_sage3():
    from sageattn3 import sageattn3_blackwell

    q, k, v = inputs()
    ref = reference(q, k, v)
    sageattn3_blackwell(q, k, v)
    out, ms = timed(lambda: sageattn3_blackwell(q, k, v))
    print(f"     sageattn3 {ms:.2f} ms")
    # FP4 attention trades more accuracy for speed than SageAttention 2.
    return compare("sageattention 3", out, ref, 0.95)


def test_flash():
    from flash_attn import flash_attn_func

    q, k, v = inputs()
    ref = reference(q, k, v)
    # flash_attn expects (batch, seq, heads, dim).
    qt, kt, vt = (t.transpose(1, 2).contiguous() for t in (q, k, v))
    flash_attn_func(qt, kt, vt)
    out, ms = timed(lambda: flash_attn_func(qt, kt, vt))
    print(f"     flash_attn {ms:.2f} ms")
    return compare("flash-attention", out.transpose(1, 2), ref, 0.999)


def test_nunchaku():
    # Needs nunchaku's Python dependencies (accelerate, diffusers, transformers...).
    from nunchaku.models.linear import SVDQW4A4Linear
    from nunchaku.ops.gemv import awq_gemv_w4a16_cuda

    ok = True
    # AWQ GEMV with all-zero packed weights, scale 1 and zero point 1: every
    # output equals the sum of the input.
    m, n, k, group = 1, 512, 1024, 64
    x = torch.randn(m, k, device=DEVICE, dtype=torch.float16)
    kernel = torch.zeros(n // 4, k // 2, dtype=torch.int32, device=DEVICE)
    ones = torch.ones(k // group, n, device=DEVICE, dtype=torch.float16)
    out = awq_gemv_w4a16_cuda(x, kernel, ones, ones, m, n, k, group).float()
    expected = torch.full_like(out, x.float().sum().item())
    gemv_ok = torch.allclose(out, expected, rtol=1e-2, atol=0.2)
    print(f"{'PASS' if gemv_ok else 'FAIL'} nunchaku gemv_awq: out={out[0, 0].item():.3f} expected={expected[0, 0].item():.3f}")
    ok &= gemv_ok

    # SVDQuant W4A4 layer (activation quantize + GEMM kernels) with zero
    # weights and bias 0.5: the output must be exactly the bias. NVFP4 runs
    # only on Blackwell.
    precisions = ["int4"] + (["nvfp4"] if torch.cuda.get_device_capability()[0] >= 10 else [])
    for precision in precisions:
        layer = SVDQW4A4Linear(3072, 3072, rank=32, precision=precision, torch_dtype=DTYPE, device=DEVICE)
        with torch.no_grad():
            for param in layer.parameters():
                param.view(torch.uint8).zero_() if param.dtype == torch.float8_e4m3fn else param.zero_()
            layer.smooth_factor.fill_(1)
            layer.wscales.copy_(torch.ones_like(layer.wscales, dtype=torch.float32).to(layer.wscales.dtype))
            layer.bias.fill_(0.5)
            y = layer(torch.randn(1, 1024, 3072, device=DEVICE, dtype=DTYPE)).float()
        layer_ok = torch.isfinite(y).all().item() and torch.allclose(y, torch.full_like(y, 0.5), atol=1e-2)
        print(f"{'PASS' if layer_ok else 'FAIL'} nunchaku W4A4 {precision}: mean={y.mean().item():.4f} expected=0.5")
        ok &= layer_ok
    return ok


TESTS = {"sage2": test_sage2, "sage3": test_sage3, "flash": test_flash, "nunchaku": test_nunchaku}

if __name__ == "__main__":
    print(f"torch {torch.__version__} | CUDA {torch.version.cuda} | {torch.cuda.get_device_name(0)}")
    selected = sys.argv[1:] or list(TESTS)
    results = {}
    for name in selected:
        try:
            results[name] = TESTS[name]()
        except Exception as err:  # noqa: BLE001 - report every failure, keep going
            print(f"FAIL {name}: {type(err).__name__}: {err}")
            results[name] = False
    sys.exit(0 if all(results.values()) else 1)
