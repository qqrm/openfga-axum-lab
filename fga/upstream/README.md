# Historical Lakekeeper model

The active model is `../model.json`, the attributed, modified Openlane subset.
The files named `lakekeeper-*` and `lakekeeper.LICENSE` / `lakekeeper.NOTICE`
preserve the exact model and attribution from repository commit
`609967a2cf56f7070d6edd3fa55163f18752f47b` (PR #24). They are retained for audit
and comparison with historical configuration; the Openlane bootstrap does not
load this model or convert its catalog objects.

`lakekeeper-provenance.json` describes `lakekeeper-model.json`, not the active
Openlane model. Its original upstream commit, digest and Apache-2.0 notices
remain unchanged. Historical catalog SQL is retained separately; a populated
catalog or a bound catalog authorization migration requires an explicit cutover
and is refused by the Openlane demo/import path.
