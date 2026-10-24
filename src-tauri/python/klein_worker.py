"""RapidOne's AI worker for the Remove tool (`klein.rs`): started once and
kept running, so the models load once.

- Klein: FLUX.2 Klein 4B (Apache-2.0), removes what it's asked to.
- SAM 3 (Meta's SAM License; optional): finds objects from words or a box
  and outlines them exactly. Loaded the first time it's asked for.

  python -I klein_worker.py <Klein model folder> [<SAM 3 checkpoint .safetensors>]

Talks JSON, one line each way. Jobs:
  {"op": "edit", "input": "...png", "output": "...png", "prompt": "...", "width": 1024, "height": 768}
  {"op": "find", "image": "...jpg", "words": "sign", "out": "<folder>", "stem": "..."}
  {"op": "prepare", "image": "...jpg"}  (SAM 3 analyses it now; finds on it are then quick)
  {"op": "rorem", "model": "<RORem diffusers folder>", "input": "...png", "mask": "...png",
   "output": "...png", "width": 1024, "height": 768, "seed": 0}  (Medium: RORem fills the mask)
  {"op": "find", "image": "...jpg", "box": [x0, y0, x1, y1], "out": "<folder>", "stem": "..."}
Replies {"ok": true, ...} or {"ok": false, "error": "..."}; "find" gives
{"ok": true, "found": [{"mask": "...png", "box": [x0, y0, x1, y1], "score": 0.94}, ...]}
(masks are white-on-black PNGs the size of the image, boxes in its pixels; for
a box prompt only the object that best fills the box is returned).
The first line out is {"ready": true} (or {"ok": false, ...} if it can't start).

GPU memory (17 Oct): with room on the card (13 GB free at start by the
driver's count, nvidia-smi: Lightroom closed), Klein's transformer and VAE stay on the GPU and its text encoder
runs on the CPU (~1.4 s a sentence on a Ryzen 9950X3D), each sentence
encoded once and kept - so a removal moves nothing between CPU and GPU.
With less room (or if the card fills up later), the old way: text encoder
and transformer take turns on the GPU (both together need over 16 GB).
"""
import json
import os
import queue
import sys
import threading
import time
import traceback
import types

# With no job for this long, the worker ends (its GPU memory freed); RapidOne
# starts a new one when it's next needed.
IDLE_EXIT = 15 * 60


def say(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def load_sam(checkpoint):
    # Triton (GPU kernels) is Linux-only; sam3 imports one for its video
    # tracker (sam3.model.edt), which still images never use.
    edt = types.ModuleType("sam3.model.edt")

    def no_triton(*a, **k):
        raise RuntimeError("needs Triton (video tracking only)")

    edt.edt_triton = no_triton
    sys.modules.setdefault("sam3.model.edt", edt)
    import torch
    from safetensors.torch import load_file
    from sam3.model.sam3_image_processor import Sam3Processor
    from sam3.model_builder import build_sam3_image_model

    model = build_sam3_image_model(load_from_HF=False, checkpoint_path=None, device="cuda")
    weights = {k.replace("detector.", ""): v.float() for k, v in load_file(checkpoint).items() if k.startswith("detector.")}
    model.load_state_dict(weights, strict=False)
    return Sam3Processor(model.cuda().eval())


# SAM 3's analysis of the last picture (the slow part), kept so each box or
# words on the same photo only runs the quick prompt step.
CACHED = {"key": None, "state": None, "size": None}


def analysed(sam, path):
    import torch
    from PIL import Image

    key = (path, os.path.getmtime(path))
    if CACHED["key"] == key:
        sam.reset_all_prompts(CACHED["state"])
        return CACHED["state"], CACHED["size"]
    CACHED.update(key=None, state=None, size=None)
    img = Image.open(path).convert("RGB")
    with torch.autocast("cuda", dtype=torch.bfloat16):
        state = sam.set_image(img)
    CACHED.update(key=key, state=state, size=img.size)
    return state, img.size


# Free GPU memory at start for Klein to stay on the card: transformer + VAE
# (7.5 GB), SAM 3 (3.4 GB) and room to work.
RESIDENT_FREE = 13e9
# While resident, less free than this before a removal: switch to taking turns.
LOW_FREE = 1.2e9

def gpu_free():
    """Free memory on the card, in bytes, as the driver counts it (nvidia-smi:
    every program's use, Lightroom's included). On Windows PyTorch's own figure
    leaves other programs out, and a full card there doesn't fail - it spills
    into system RAM and crawls - so it can't be trusted for this."""
    import subprocess

    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=memory.free", "--format=csv,noheader,nounits"],
                             capture_output=True, text=True, timeout=5,
                             creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout
        return int(out.split()[0]) * 2**20
    except Exception:  # noqa: BLE001 - no nvidia-smi: PyTorch's figure
        import torch

        return torch.cuda.mem_get_info()[0]


# RORem (Medium): loaded the first time it's asked for. Only one big model on
# the card at a time: Klein's transformer leaves while RORem works, and comes
# back for Klein's next job.
RO = {"pipe": None, "path": None}
# RORem's working size (long edge; it was trained at 512, its card says
# 15-30 steps work as well as 50, guidance 1 = no prompt). 512: clean fills on
# the 17 Oct test (bride, guest; examples/remove_compare.rs); 768 was no
# better. RAPIDONE_RO_SIDE / _STEPS override it for tests.
RO_SIDE = int(os.environ.get("RAPIDONE_RO_SIDE", 512))
RO_STEPS = int(os.environ.get("RAPIDONE_RO_STEPS", 24))


def rorem_job(job, pipe, resident):
    import torch
    from diffusers import StableDiffusionXLInpaintPipeline
    from PIL import Image

    if RO["pipe"] is None or RO["path"] != job["model"]:
        RO["pipe"] = StableDiffusionXLInpaintPipeline.from_pretrained(job["model"], torch_dtype=torch.float16)
        RO["pipe"].set_progress_bar_config(disable=True)
        RO["path"] = job["model"]
    ro = RO["pipe"]
    if resident:
        pipe.transformer.to("cpu")
        pipe.vae.to("cpu")
        torch.cuda.empty_cache()
    if gpu_free() > 8e9:
        ro.to("cuda")
    else:
        ro.enable_model_cpu_offload()
    w, h = int(job["width"]), int(job["height"])
    s = RO_SIDE / max(w, h)
    rw, rh = max(64, round(w * s / 8) * 8), max(64, round(h * s / 8) * 8)
    image = Image.open(job["input"]).convert("RGB").resize((rw, rh), Image.LANCZOS)
    mask = Image.open(job["mask"]).convert("L").resize((rw, rh), Image.NEAREST)
    out = ro(prompt="", image=image, mask_image=mask, width=rw, height=rh, num_inference_steps=RO_STEPS,
             guidance_scale=1.0, strength=0.99, generator=torch.Generator("cpu").manual_seed(int(job.get("seed", 0)))).images[0]
    out.resize((w, h), Image.LANCZOS).save(job["output"])
    ro.to("cpu")
    torch.cuda.empty_cache()
    if resident:
        pipe.transformer.to("cuda")
        pipe.vae.to("cuda")


# Each sentence's embedding (CPU-encoded once), by sentence.
EMBEDS = {}


def embeds(text_encoder, tokenizer, prompt):
    import torch
    from diffusers import Flux2KleinPipeline

    if prompt not in EMBEDS:
        with torch.inference_mode():
            EMBEDS[prompt] = Flux2KleinPipeline._get_qwen3_prompt_embeds(text_encoder, tokenizer, [prompt], device="cpu")
    return EMBEDS[prompt].to("cuda")


def find(sam, job):
    import numpy as np
    import torch
    from PIL import Image

    state, (w, h) = analysed(sam, job["image"])
    with torch.autocast("cuda", dtype=torch.bfloat16):
        if job.get("box"):
            x0, y0, x1, y1 = job["box"]
            res = sam.add_geometric_prompt([(x0 + x1) / 2 / w, (y0 + y1) / 2 / h, (x1 - x0) / w, (y1 - y0) / h], True, state)
        else:
            res = sam.set_text_prompt(state=state, prompt=job["words"])
    found = []
    for i, (m, b, s) in enumerate(zip(res["masks"], res["boxes"], res["scores"])):
        found.append((m.squeeze().cpu().numpy(), [float(v) for v in b], float(s)))
    if job.get("box") and found:
        # A box is an example of the concept, so SAM 3 may find others like
        # it: keep the one that best fills the box (overlap x share inside).
        x0, y0, x1, y1 = [int(round(v)) for v in job["box"]]
        inside = np.zeros((h, w), bool)
        inside[max(y0, 0):y1, max(x0, 0):x1] = True

        def fit(f):
            m = f[0]
            inter = (m & inside).sum()
            return inter / max((m | inside).sum(), 1) * inter / max(m.sum(), 1)

        best = max(found, key=fit)
        found = [best] if fit(best) > 0.05 else []
    # Most confident first.
    found.sort(key=lambda f: -f[2])
    out = []
    for i, (m, b, s) in enumerate(found):
        path = os.path.join(job["out"], f"{job['stem']}-mask{i}.png")
        Image.fromarray(m.astype(np.uint8) * 255).save(path)
        out.append({"mask": path.replace("\\", "/"), "box": b, "score": round(s, 3)})
    return out


def main():
    try:
        import torch
        from diffusers import Flux2KleinPipeline
        from PIL import Image

        pipe = Flux2KleinPipeline.from_pretrained(sys.argv[1], torch_dtype=torch.bfloat16)
        pipe.set_progress_bar_config(disable=True)
        text_encoder = pipe.text_encoder
        resident = gpu_free() > RESIDENT_FREE
        if resident:
            pipe.transformer.to("cuda")
            pipe.vae.to("cuda")
            # Off the pipeline, so it runs on the GPU; prompts come as embeddings.
            pipe.register_modules(text_encoder=None)
        else:
            pipe.enable_model_cpu_offload()
    except Exception as e:  # noqa: BLE001 - reported to RapidOne
        say({"ok": False, "error": f"Klein couldn't start: {e}"})
        return
    sam_checkpoint = sys.argv[2] if len(sys.argv) > 2 else None
    sam = None
    say({"ready": True})
    # Lines read on a thread, so an idle worker can notice and end; input
    # closing (RapidOne quit or gone) ends it too.
    lines = queue.Queue()

    def read():
        for line in sys.stdin:
            lines.put(line)
        lines.put(None)

    threading.Thread(target=read, daemon=True).start()
    while True:
        try:
            line = lines.get(timeout=IDLE_EXIT)
        except queue.Empty:
            break
        if line is None:
            break
        line = line.strip()
        if not line:
            continue
        try:
            job = json.loads(line)
            t = time.time()
            if job.get("op") in ("find", "prepare"):
                if not sam_checkpoint:
                    raise RuntimeError("SAM 3 isn't set up on this computer")
                if sam is None:
                    sam = load_sam(sam_checkpoint)
                if job["op"] == "prepare":
                    analysed(sam, job["image"])
                    say({"ok": True, "seconds": round(time.time() - t, 2)})
                    continue
                found = find(sam, job)
                say({"ok": True, "found": found, "seconds": round(time.time() - t, 2)})
                continue
            if job.get("op") == "rorem":
                rorem_job(job, pipe, resident)
                say({"ok": True, "seconds": round(time.time() - t, 2)})
                continue
            image = Image.open(job["input"]).convert("RGB")
            args = dict(
                image=image,
                width=int(job["width"]),
                height=int(job["height"]),
                num_inference_steps=int(job.get("steps", 4)),
                generator=torch.Generator("cpu").manual_seed(int(job.get("seed", 0))),
            )
            out = None
            if resident and gpu_free() < LOW_FREE:
                # Another program has taken the room (Lightroom opened?): take
                # turns from now on, before the card spills into system RAM.
                resident = False
                pipe.register_modules(text_encoder=text_encoder)
                pipe.transformer.to("cpu")
                pipe.vae.to("cpu")
                torch.cuda.empty_cache()
                pipe.enable_model_cpu_offload()
            if resident:
                try:
                    out = pipe(prompt_embeds=embeds(text_encoder, pipe.tokenizer, job["prompt"]), **args).images[0]
                except torch.cuda.OutOfMemoryError:
                    # The card filled up (Lightroom opened?): take turns from now on.
                    resident = False
                    pipe.register_modules(text_encoder=text_encoder)
                    pipe.transformer.to("cpu")
                    pipe.vae.to("cpu")
                    torch.cuda.empty_cache()
                    pipe.enable_model_cpu_offload()
                    args["generator"] = torch.Generator("cpu").manual_seed(int(job.get("seed", 0)))
            if out is None:
                out = pipe(prompt=job["prompt"], **args).images[0]
            out.save(job["output"])
            say({"ok": True, "seconds": round(time.time() - t, 2)})
        except Exception as e:  # noqa: BLE001 - reported to RapidOne
            traceback.print_exc(file=sys.stderr)
            say({"ok": False, "error": str(e)})


if __name__ == "__main__":
    main()
