#!/usr/bin/env python3
"""Generate Rust types for Codex's app-server protocol from its JSON schema.

Usage: scripts/gen-codex-types.py <out_dir>

Needs `codex` (the pinned version) and `cargo-typify` on PATH. Writes
<out_dir>/schema/codex-app-server.schema.json (committed, so a version bump
shows a schema diff) and <out_dir>/src/generated.rs.
"""
import json, pathlib, re, subprocess, sys, tempfile

# Definitions typify cannot represent; they become serde_json::Value.
OPAQUE = ["CodexErrorInfo", "ThreadItemsListCursor", "McpServerElicitationRequestParams"]
# Top-level message enums the v2 bundle leaves out, plus the responses to
# server requests (approvals and friends), which have their own files.
EXTRA_TOPS = ["ServerRequest", "ClientNotification"]
EXTRA_SUFFIX = "Response"

def variants(defs, top):
    """(method, params type or None) for each variant of a top-level union."""
    for v in defs[top]["oneOf"]:
        props = v["properties"]
        params = props.get("params", {}).get("$ref", "").rsplit("/", 1)[-1] or None
        yield props["method"]["enum"][0], params


def camel(method):
    return "".join(p[:1].upper() + p[1:] for p in re.split(r"[/_]", method) if p)


def gen_enum(defs, top, name, doc):
    vs = list(variants(defs, top))
    lines = [f"/// {doc}", "#[derive(Debug, Clone)]", f"pub enum {name} {{"]
    for m, p in vs:
        lines.append(f"    /// `{m}`")
        lines.append(f"    {camel(m)}({p})," if p else f"    {camel(m)},")
    lines += ["    /// A method this version of the types does not know.",
              "    Unknown { method: String, params: serde_json::Value },", "}", "",
              f"impl {name} {{",
              "    pub fn parse(method: &str, params: serde_json::Value) -> Result<Self, serde_json::Error> {",
              "        Ok(match method {"]
    for m, p in vs:
        lines.append(f'            "{m}" => Self::{camel(m)}(serde_json::from_value(params)?),' if p
                     else f'            "{m}" => Self::{camel(m)},')
    lines += ["            _ => Self::Unknown { method: method.to_owned(), params },",
              "        })", "    }", "", "    pub fn method(&self) -> &str {", "        match self {"]
    for m, p in vs:
        lines.append(f'            Self::{camel(m)}{"(_)" if p else ""} => "{m}",')
    lines += ["            Self::Unknown { method, .. } => method,", "        }", "    }", "}", ""]
    return "\n".join(lines)


def gen_dispatch(defs):
    out = ["//! Method-keyed dispatch over the generated types.", "",
           "#![allow(clippy::large_enum_variant)]", "", "use super::generated::*;", "",
           "/// A client-to-server request: params type, method name and response type.",
           "pub trait Request: serde::Serialize {",
           "    const METHOD: &'static str;",
           "    type Response: serde::de::DeserializeOwned;", "}", ""]
    seen = {}
    for m, p in variants(defs, "ClientRequest"):
        if p:
            seen.setdefault(p, []).append(m)
    for p, ms in sorted(seen.items()):
        if len(ms) != 1:
            continue  # params type shared by several methods: no single impl
        resp = p.removesuffix("Params") + "Response"
        resp = resp if resp in defs and "opaque" not in defs[resp].get("description", "") else "serde_json::Value"
        out.append(f'impl Request for {p} {{\n    const METHOD: &\'static str = "{ms[0]}";\n    type Response = {resp};\n}}')
    out.append("")
    out.append(gen_enum(defs, "ServerNotification", "Notification", "A server-to-client notification."))
    out.append(gen_enum(defs, "ServerRequest", "IncomingRequest", "A server-to-client request (approvals and friends); answer it by id."))
    return "\n".join(out)


out = pathlib.Path(sys.argv[1])
version = subprocess.run(["codex", "--version"], capture_output=True, text=True, check=True).stdout.strip()
with tempfile.TemporaryDirectory() as tmp:
    tmp = pathlib.Path(tmp)
    subprocess.run(["codex", "app-server", "generate-json-schema", "--out", str(tmp)], check=True)
    bundle = json.loads((tmp / "codex_app_server_protocol.v2.schemas.json").read_text())
    defs = bundle["definitions"]
    tops = EXTRA_TOPS + sorted(
        f.stem for f in tmp.glob("*.json")
        if f.stem.endswith(EXTRA_SUFFIX) and f.stem not in defs
    )
    for name in tops:
        top = json.loads((tmp / f"{name}.json").read_text())
        sub = top.pop("definitions", {})
        top.pop("$schema", None)
        defs.setdefault(name, top)
        todo = [top]
        while todo:
            for ref in set(re.findall(r'"#/definitions/([^"]+)"', json.dumps(todo.pop()))):
                if ref not in defs and ref in sub:
                    defs[ref] = sub[ref]
                    todo.append(sub[ref])
    bundle["title"] = f"Codex app-server protocol ({version})"
    (out / "schema").mkdir(parents=True, exist_ok=True)
    schema_path = out / "schema" / "codex-app-server.schema.json"
    schema_path.write_text(json.dumps(bundle, indent=1, sort_keys=True) + "\n")

    for name in OPAQUE:
        defs[name] = {"description": "Opaque: not representable by typify; kept as raw JSON."}
    typify_in = tmp / "typify.json"
    typify_in.write_text(json.dumps(bundle))
    rs = out / "src" / "generated.rs"
    subprocess.run(["cargo", "typify", "--no-builder", "-o", str(rs), str(typify_in)], check=True)
    body = rs.read_text()
    rs.write_text(f"// @generated by scripts/gen-codex-types.py from {version}. Do not edit.\n" + body)
    dispatch = out / "src" / "dispatch.rs"
    dispatch.write_text(f"// @generated by scripts/gen-codex-types.py from {version}. Do not edit.\n" + gen_dispatch(defs))
print(f"generated from {version}")
