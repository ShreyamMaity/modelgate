import argparse
import inspect
import json
import os
import shutil

import onnx
import torch
from onnx import numpy_helper
from onnxruntime.quantization import QuantType, quantize_dynamic
from transformers import AutoModelForTokenClassification, AutoTokenizer

SCRIPTS = [
    (0x20, 0x24F), (0x250, 0x36F), (0x900, 0xDFF), (0x1E00, 0x1EFF), (0x2000, 0x2BFF),
    (0x200B, 0x200D), (0xA8E0, 0xA8FF), (0xFE0F, 0xFE0F), (0x1F300, 0x1FAFF),
]


def export(model_id, out):
    tok = AutoTokenizer.from_pretrained(model_id, use_fast=True)
    model = AutoModelForTokenClassification.from_pretrained(model_id).eval()
    enc = tok(["Asha Rao lives in Pune"], return_tensors="pt")
    accepted = inspect.signature(model.forward).parameters
    names = [k for k in ("input_ids", "attention_mask", "token_type_ids") if k in enc and k in accepted]

    class Wrap(torch.nn.Module):
        def __init__(self, inner):
            super().__init__()
            self.inner = inner

        def forward(self, *args):
            return self.inner(**dict(zip(names, args))).logits

    axes = {k: {0: "b", 1: "t"} for k in names}
    axes["logits"] = {0: "b", 1: "t"}
    torch.onnx.export(Wrap(model), tuple(enc[k] for k in names), f"{out}/model.onnx", input_names=names,
                      output_names=["logits"], dynamic_axes=axes, opset_version=17, dynamo=False)
    model.config.to_json_file(f"{out}/config.json")
    tok.save_pretrained(f"{out}/tok")
    shutil.copy(f"{out}/tok/tokenizer.json", f"{out}/tokenizer.json")
    shutil.rmtree(f"{out}/tok")


def keep_piece(piece):
    return all(ch == "▁" or any(a <= ord(ch) <= b for a, b in SCRIPTS) for ch in piece)


def prune(out):
    tok = json.load(open(f"{out}/tokenizer.json"))
    if tok["model"]["type"] != "Unigram":
        return
    vocab = tok["model"]["vocab"]
    special = {t["id"] for t in tok["added_tokens"]} | {tok["model"]["unk_id"]}
    keep = [i for i, (p, _) in enumerate(vocab) if i in special or keep_piece(p)]
    new = {old: n for n, old in enumerate(keep)}
    tok["model"]["vocab"] = [vocab[i] for i in keep]
    tok["model"]["unk_id"] = new[tok["model"]["unk_id"]]
    for t in tok["added_tokens"]:
        t["id"] = new[t["id"]]

    def remap(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k == "ids" and isinstance(v, list) and all(isinstance(x, int) for x in v):
                    o[k] = [new[x] for x in v]
                else:
                    remap(v)
        elif isinstance(o, list):
            for x in o:
                remap(x)

    remap(tok.get("post_processor"))
    json.dump(tok, open(f"{out}/tokenizer.json", "w"), ensure_ascii=False)
    graph = onnx.load(f"{out}/model_int8.onnx")
    for init in graph.graph.initializer:
        arr = numpy_helper.to_array(init)
        if "word_embeddings" in init.name and arr.ndim == 2 and arr.shape[0] == len(vocab):
            init.CopyFrom(numpy_helper.from_array(arr[keep], init.name))
    onnx.save(graph, f"{out}/model_int8.onnx")
    print(f"vocab {len(vocab)} -> {len(keep)}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("model_id")
    ap.add_argument("out")
    ap.add_argument("--prune", action="store_true")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    export(a.model_id, a.out)
    quantize_dynamic(f"{a.out}/model.onnx", f"{a.out}/model_int8.onnx", weight_type=QuantType.QInt8)
    if a.prune:
        prune(a.out)
    os.remove(f"{a.out}/model.onnx")
    print({f: os.path.getsize(f"{a.out}/{f}") for f in os.listdir(a.out)})


if __name__ == "__main__":
    main()
