"use strict";
const $ = id => document.getElementById(id);
function expression(value) {
  if (value.this) return "direct assignment";
  if (value.computedUserset) return value.computedUserset.relation;
  if (value.tupleToUserset) return `${value.tupleToUserset.computedUserset.relation} from ${value.tupleToUserset.tupleset.relation}`;
  if (value.union) return value.union.child.map(expression).join("\nOR ");
  if (value.intersection) return value.intersection.child.map(v => `(${expression(v)})`).join("\nAND ");
  if (value.difference) return `(${expression(value.difference.base)})\nBUT NOT (${expression(value.difference.subtract)})`;
  throw new Error("Unrecognized model expression");
}
async function json(path) {
  const response = await fetch(path);
  if (!response.ok) throw new Error(`Cannot load ${path}: HTTP ${response.status}`);
  return response.json();
}
async function start() {
  const [model, provenance] = await Promise.all([json("model.json"), json("provenance.json")]);
  $("provenance").textContent = `${provenance.project} v${provenance.version} · ${provenance.commit.slice(0, 8)} · ${provenance.license}`;
  if (provenance.source_url.startsWith("https://github.com/lakekeeper/lakekeeper/")) $("upstream").href = provenance.source_url;
  for (const type of model.type_definitions) {
    const option = document.createElement("option"); option.value = type.type; option.textContent = type.type; $("model-type").append(option);
  }
  $("model-type").value = "lakekeeper_table";
  function showRelation() {
    const type = model.type_definitions.find(type => type.type === $("model-type").value);
    const name = $("model-relation").value, relation = type.relations?.[name];
    $("relation-expression").textContent = relation ? `${name} =\n${expression(relation)}` : "No relations on this type.";
    const subjects = type.metadata?.relations?.[name]?.directly_related_user_types || [];
    $("direct-types").textContent = subjects.length ? "Assignable subjects: " + subjects.map(subject => subject.type + (subject.relation ? "#" + subject.relation : subject.wildcard ? ":*" : "")).join(", ") : "Computed actions are checked, not directly assigned.";
  }
  function showType() {
    const type = model.type_definitions.find(type => type.type === $("model-type").value);
    const names = Object.keys(type.relations || {}).sort();
    $("model-relation").replaceChildren(...names.map(name => { const option = document.createElement("option"); option.value = name; option.textContent = name; return option; }));
    if (names.includes("can_read_data")) $("model-relation").value = "can_read_data";
    $("relation-count").textContent = `${names.length} relations · ${model.type_definitions.length} types in the original model`;
    showRelation();
  }
  $("model-type").addEventListener("change", showType); $("model-relation").addEventListener("change", showRelation); showType();
  const response = await fetch("domain.wasm");
  if (!response.ok) throw new Error("Cannot load Rust lifecycle kernel");
  const { instance } = await WebAssembly.instantiate(await response.arrayBuffer());
  if (instance.exports.next_state(1, 1) !== 2 || instance.exports.is_visible(2) !== 0) throw new Error("Lifecycle invariant failed");
  $("lifecycle").textContent = "Compiled Rust check passed: deleting objects are hidden; a deleted object cannot be provisioned again. This checks lifecycle rules, not OpenFGA permissions.";
}
start().catch(error => { $("error").textContent = error.message; });
