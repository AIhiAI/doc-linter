//! SQLite impl of [`GraphRead`]: the typed query APIs of
//! `store::query::*` as SQL over the store DDL. Row mapping mirrors the
//! the store functions one for one (same projection structs, same Rust-side
//! ranking and dedup); only the statement text differs. Traversals use
//! recursive CTEs with a depth cap (`query_impact`, `shortest_path`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use super::SqliteDb;
use crate::config::LintConfig;
use crate::endpoint_extract::EndpointKind;
use crate::graph::EdgeKind;
use crate::graph_read::GraphRead;
use crate::ids::{DocId, EntityId, FunctionSymbol};
use crate::store::query::{
    AtCoveringDoc, AtEndpoint, AtEntityLink, EgoEdge, EgoNode, EntityEgoGraph, FileRow,
    ImpactDepthLevel, ImpactEndpoint, ImpactEntity, ModuleRow,
};
use crate::store::{
    edge_label_to_kebab, typed, AtQueryResult, ConfidenceTier, CoveringDocRow, DarkEndpointRow,
    DarkFunctionRow, DeadCodeRow, DocRow, EdgeType, EndpointRow, EntityGapRow, EntityRow,
    EntitySubgraph, EntitySubgraphEdge, EntitySubgraphNode, FunctionContext, FunctionFull,
    FunctionMentionRow, FunctionNeighbour, FunctionRow, ImpactResult, LinkRow, PathStep,
    RankedEntityRow, SiblingRow, StoreRead, Value,
};

/// Doc -> Doc edge tables (the `is_doc_doc` rows of the the store edge schema).
const DOC_DOC: &[&str] = &[
    "WIKILINK",
    "MD_LINK",
    "DEPENDS_ON",
    "INFORMED_BY",
    "SUPERSEDES",
    "CRATE_REF",
];

fn s(v: &Value) -> String {
    v.str_or_empty()
}

/// Non-empty string cell, as `val_string_opt`.
fn so(v: &Value) -> Option<String> {
    v.as_str().filter(|x| !x.is_empty()).map(str::to_owned)
}

fn u32v(v: &Value) -> u32 {
    v.as_i64().map_or(0, |n| n as u32)
}

fn flag(v: &Value) -> bool {
    matches!(v, Value::Int(1) | Value::Bool(true))
}

fn text(x: &str) -> Value {
    Value::Str(x.to_string())
}

impl SqliteDb {
    fn q(&self, sql: &str, params: &[(&str, Value)]) -> Result<Vec<Vec<Value>>> {
        StoreRead::query(self, sql, params)
    }

    /// `owner -> values` of a side table, in insertion order.
    fn list_map(&self, table: &str, owner: &str) -> Result<HashMap<String, Vec<String>>> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for r in self.q(
            &format!("SELECT {owner}, value FROM {table} ORDER BY rowid"),
            &[],
        )? {
            m.entry(s(&r[0])).or_default().push(s(&r[1]));
        }
        Ok(m)
    }

    fn doc_rows(&self, where_id: Option<&str>) -> Result<Vec<DocRow>> {
        let sql = format!(
            "SELECT id, path, role, kind, lifecycle, title, summary, status, updated FROM Doc{} \
             ORDER BY id",
            if where_id.is_some() {
                " WHERE id = $id"
            } else {
                ""
            }
        );
        let params: Vec<(&str, Value)> = where_id.map(|i| ("id", text(i))).into_iter().collect();
        let rows = self.q(&sql, &params)?;
        let (mut tags, mut covers, mut attrs) = (
            self.list_map("doc_tags", "doc_id")?,
            self.list_map("doc_covers", "doc_id")?,
            self.list_map("doc_attributes", "doc_id")?,
        );
        Ok(rows
            .iter()
            .map(|r| {
                let id = s(&r[0]);
                DocRow {
                    tags: tags.remove(&id).unwrap_or_default(),
                    covers: covers.remove(&id).unwrap_or_default(),
                    attributes: attrs.remove(&id).unwrap_or_default(),
                    path: s(&r[1]),
                    role: s(&r[2]),
                    kind: so(&r[3]),
                    lifecycle: so(&r[4]),
                    title: s(&r[5]),
                    summary: s(&r[6]),
                    status: s(&r[7]),
                    updated: s(&r[8]),
                    id,
                }
            })
            .collect())
    }

    fn link_rows(&self, id: &DocId, outbound: bool) -> Result<Vec<LinkRow>> {
        // outbound: this doc is `src`, the neighbour is `dst`; inbound mirrors.
        let (me, other) = if outbound {
            ("src", "dst")
        } else {
            ("dst", "src")
        };
        let sql = DOC_DOC
            .iter()
            .map(|t| {
                format!(
                    "SELECT b.id, b.title, b.role, r.line, '{t}' FROM \"{t}\" r \
                     JOIN Doc b ON b.id = r.{other} WHERE r.{me} = $id"
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let mut rows: Vec<LinkRow> = self
            .q(&sql, &[("id", text(id.as_str()))])?
            .iter()
            .map(|r| LinkRow {
                id: s(&r[0]),
                title: s(&r[1]),
                role: s(&r[2]),
                line: u32v(&r[3]) as usize,
                edge_type: edge_label_to_kebab(&s(&r[4])),
            })
            .collect();
        if outbound {
            for r in self.q(
                "SELECT e.id, e.display, r.line FROM \"COVERS\" r \
                 JOIN Entity e ON e.id = r.dst WHERE r.src = $id",
                &[("id", text(id.as_str()))],
            )? {
                rows.push(LinkRow {
                    id: format!("entity-{}", s(&r[0])),
                    title: s(&r[1]),
                    role: "ontology-entity".to_string(),
                    line: u32v(&r[2]) as usize,
                    edge_type: "covers".to_string(),
                });
            }
        }
        Ok(rows)
    }

    fn function_full_rows(&self, sql: &str, params: &[(&str, Value)]) -> Result<Vec<FunctionFull>> {
        Ok(self
            .q(sql, params)?
            .iter()
            .map(|r| FunctionFull {
                symbol: s(&r[0]),
                crate_name: s(&r[1]),
                file: s(&r[2]),
                line: u32v(&r[3]),
                doc_comment: s(&r[4]),
            })
            .collect())
    }

    fn dark_rows(&self, sql: &str, params: &[(&str, Value)]) -> Result<Vec<DarkFunctionRow>> {
        Ok(self
            .q(sql, params)?
            .iter()
            .map(|r| DarkFunctionRow {
                crate_name: s(&r[0]),
                symbol: s(&r[1]),
                file: s(&r[2]),
                line: u32v(&r[3]),
            })
            .collect())
    }
}

const DARK_FN: &str = "SELECT f.crate, f.symbol, f.file, f.line FROM Function f \
    WHERE coalesce(f.generated, 0) = 0 AND (f.doc_comment IS NULL OR f.doc_comment = '') \
      AND NOT EXISTS (SELECT 1 FROM \"FUNCTION_MENTIONS\" m JOIN Entity e ON e.id = m.dst \
                      WHERE m.src = f.symbol)";

const NEIGHBOUR_ENTITY: &str = "b.id, b.display, b.mention_count, b.is_god_node";

fn entity_node(r: &[Value]) -> EntitySubgraphNode {
    EntitySubgraphNode {
        id: s(&r[0]),
        display: s(&r[1]),
        mention_count: r[2].as_u64_or_zero(),
        is_god_node: flag(&r[3]),
    }
}

impl GraphRead for SqliteDb {
    fn list_all_docs(&self) -> Result<Vec<DocRow>> {
        self.doc_rows(None)
    }

    fn get_doc(&self, id: &DocId) -> Result<Option<DocRow>> {
        Ok(self.doc_rows(Some(id.as_str()))?.into_iter().next())
    }

    fn outbound(&self, id: &DocId) -> Result<Vec<LinkRow>> {
        self.link_rows(id, true)
    }

    fn inbound(&self, id: &DocId) -> Result<Vec<LinkRow>> {
        self.link_rows(id, false)
    }

    fn covers_inbound_for_entity_doc(&self, id: &DocId) -> Result<Vec<LinkRow>> {
        let Some(entity_id) = crate::store::query::docs::entity_id_from_entity_doc(id.as_str())
        else {
            return Ok(Vec::new());
        };
        Ok(self
            .q(
                "SELECT d.id, d.title, d.role, r.line FROM \"COVERS\" r \
                 JOIN Doc d ON d.id = r.src WHERE r.dst = $e",
                &[("e", text(entity_id))],
            )?
            .iter()
            .map(|r| LinkRow {
                id: s(&r[0]),
                title: s(&r[1]),
                role: s(&r[2]),
                line: u32v(&r[3]) as usize,
                edge_type: "covers".to_string(),
            })
            .collect())
    }

    /// Undirected shortest Doc-Doc path. A recursive CTE with a depth cap
    /// computes the BFS level of every reachable doc (`UNION` dedupes
    /// `(node, depth)`, so cost is polynomial, not path-enumerating); the
    /// path is then walked back from `to` along edges one level closer.
    fn shortest_path(
        &self,
        from: &DocId,
        to: &DocId,
        edge_filter: Option<&HashSet<EdgeKind>>,
        max_hops: u32,
    ) -> Result<Vec<PathStep>> {
        if from == to {
            return Ok(self
                .get_doc(from)?
                .map(|d| {
                    vec![PathStep {
                        id: d.id,
                        title: d.title,
                        via_line: None,
                        via_edge_type: None,
                    }]
                })
                .unwrap_or_default());
        }
        let labels: Vec<&'static str> = match edge_filter {
            Some(set) if !set.is_empty() => set
                .iter()
                .map(EdgeType::label)
                .filter(|t| DOC_DOC.contains(t))
                .collect(),
            _ => DOC_DOC.to_vec(),
        };
        if labels.is_empty() {
            return Ok(Vec::new());
        }
        let edges = labels
            .iter()
            .map(|t| {
                format!(
                    "SELECT src AS a, dst AS b, '{t}' AS t, coalesce(line, 0) AS line FROM \"{t}\" \
                     UNION ALL SELECT dst, src, '{t}', coalesce(line, 0) FROM \"{t}\""
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let levels = self.q(
            &format!(
                "WITH RECURSIVE e(a, b, t, line) AS ({edges}), \
                 walk(node, depth) AS ( \
                   SELECT $from, 0 \
                   UNION SELECT e.b, w.depth + 1 FROM walk w JOIN e ON e.a = w.node \
                   WHERE w.depth < $max) \
                 SELECT node, min(depth) FROM walk GROUP BY node"
            ),
            &[
                ("from", text(from.as_str())),
                ("max", Value::Int(i64::from(max_hops))),
            ],
        )?;
        let dist: HashMap<String, i64> = levels
            .iter()
            .filter_map(|r| Some((r[0].as_str()?.to_string(), r[1].as_i64()?)))
            .collect();
        let Some(&total) = dist.get(to.as_str()) else {
            return Ok(Vec::new());
        };
        // Walk back: at each level pick the lowest-id neighbour one step closer.
        let mut hops: Vec<(String, String, i64)> = Vec::new(); // (node, edge table, line)
        let mut cur = to.as_str().to_string();
        for level in (1..=total).rev() {
            let nbrs = self.q(
                &format!(
                    "WITH e(a, b, t, line) AS ({edges}) \
                     SELECT b, t, line FROM e WHERE a = $cur ORDER BY b, t, line"
                ),
                &[("cur", text(&cur))],
            )?;
            let pick = nbrs
                .iter()
                .find(|r| dist.get(&s(&r[0])) == Some(&(level - 1)))
                .context("shortest_path backtrack")?;
            hops.push((cur.clone(), s(&pick[1]), pick[2].as_i64().unwrap_or(0)));
            cur = s(&pick[0]);
        }
        hops.reverse();
        let title = |id: &str| -> Result<String> {
            Ok(self
                .q("SELECT title FROM Doc WHERE id = $id", &[("id", text(id))])?
                .first()
                .map(|r| s(&r[0]))
                .unwrap_or_default())
        };
        let mut steps = vec![PathStep {
            id: from.as_str().to_string(),
            title: title(from.as_str())?,
            via_line: None,
            via_edge_type: None,
        }];
        for (node, table, line) in hops {
            steps.push(PathStep {
                title: title(&node)?,
                id: node,
                via_line: Some(line as usize),
                via_edge_type: Some(edge_label_to_kebab(&table)),
            });
        }
        Ok(steps)
    }

    fn ranked_entities(&self) -> Result<Vec<RankedEntityRow>> {
        let mut sc = self.list_map("entity_scanner_coverage", "entity_id")?;
        let mut rows: Vec<RankedEntityRow> = self
            .q(
                "SELECT id, display, mention_count, is_god_node, entity_class FROM Entity",
                &[],
            )?
            .iter()
            .filter(|r| !s(&r[0]).is_empty())
            .map(|r| {
                let id = s(&r[0]);
                RankedEntityRow {
                    rank: 0,
                    display: s(&r[1]),
                    mentions: r[2].as_u64_or_zero(),
                    god_node: flag(&r[3]),
                    scanner_coverage: sc.remove(&id).unwrap_or_default(),
                    entity_class: so(&r[4]),
                    id,
                }
            })
            .collect();
        rows.sort_by(|a, b| b.mentions.cmp(&a.mentions).then(a.id.cmp(&b.id)));
        for (i, r) in rows.iter_mut().enumerate() {
            r.rank = (i + 1) as u32;
        }
        Ok(rows)
    }

    fn get_entity(&self, id: &EntityId) -> Result<Option<EntityRow>> {
        Ok(self
            .q(
                "SELECT id, display, description FROM Entity WHERE id = $id",
                &[("id", text(id.as_str()))],
            )?
            .first()
            .map(|r| EntityRow {
                id: s(&r[0]),
                display: s(&r[1]),
                description: s(&r[2]),
            }))
    }

    fn top_entities_in_crate(&self, crate_name: &str, limit: usize) -> Result<Vec<String>> {
        Ok(self
            .q(
                "SELECT e.id, count(DISTINCT f.symbol) AS c FROM \"FUNCTION_MENTIONS\" m \
                 JOIN Function f ON f.symbol = m.src JOIN Entity e ON e.id = m.dst \
                 WHERE f.crate = $crate GROUP BY e.id ORDER BY c DESC, e.id ASC",
                &[("crate", text(crate_name))],
            )?
            .iter()
            .map(|r| s(&r[0]))
            .filter(|id| !id.is_empty())
            .take(limit)
            .collect())
    }

    fn query_entity_subgraph(&self, entity_id: &str, depth: u32) -> Result<Option<EntitySubgraph>> {
        let Some(row) = self
            .q(
                "SELECT id, display, mention_count, is_god_node FROM Entity WHERE id = $id",
                &[("id", text(entity_id))],
            )?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let centre = entity_node(&row);
        let mut visited: HashSet<String> = HashSet::from([centre.id.clone()]);
        let mut nodes = vec![centre.clone()];
        let mut frontier = vec![centre.id.clone()];
        let out = format!(
            "SELECT {NEIGHBOUR_ENTITY} FROM \"RELATES_TO\" r JOIN Entity b ON b.id = r.dst \
             WHERE r.src = $id"
        );
        let inn = format!(
            "SELECT {NEIGHBOUR_ENTITY} FROM \"RELATES_TO\" r JOIN Entity b ON b.id = r.src \
             WHERE r.dst = $id"
        );
        for _ in 0..depth {
            let mut next = Vec::new();
            for id in &frontier {
                for sql in [&out, &inn] {
                    for r in self.q(sql, &[("id", text(id))])? {
                        let n = entity_node(&r);
                        if !n.id.is_empty() && visited.insert(n.id.clone()) {
                            next.push(n.id.clone());
                            nodes.push(n);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let mut edges: Vec<EntitySubgraphEdge> = self
            .q(
                "SELECT src, dst, type, weight, frequency, source FROM \"RELATES_TO\"",
                &[],
            )?
            .iter()
            .filter(|r| visited.contains(&s(&r[0])) && visited.contains(&s(&r[1])))
            .map(|r| EntitySubgraphEdge {
                source: s(&r[0]),
                target: s(&r[1]),
                type_: s(&r[2]),
                weight: match &r[3] {
                    Value::Float(f) => *f,
                    _ => 0.0,
                },
                frequency: r[4].as_i64().unwrap_or(0),
                edge_source: s(&r[5]),
            })
            .collect();
        nodes.sort_by(|a, b| b.is_god_node.cmp(&a.is_god_node).then(a.id.cmp(&b.id)));
        edges.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then(a.target.cmp(&b.target))
                .then(a.type_.cmp(&b.type_))
        });
        Ok(Some(EntitySubgraph {
            centre: centre.id,
            depth,
            nodes,
            edges,
        }))
    }

    fn query_entity_ego_graph(
        &self,
        entity_id: &str,
        depth: u32,
    ) -> Result<Option<EntityEgoGraph>> {
        let Some(row) = self
            .q(
                "SELECT id, display, mention_count, is_god_node FROM Entity WHERE id = $id",
                &[("id", text(entity_id))],
            )?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let centre_id = s(&row[0]);
        let mut visited_entities: HashSet<String> = HashSet::from([centre_id.clone()]);
        let (mut docs, mut functions, mut endpoints) =
            (HashSet::new(), HashSet::new(), HashSet::new());
        let mut nodes = vec![EgoNode::Entity {
            id: centre_id.clone(),
            display: s(&row[1]),
            mention_count: row[2].as_u64_or_zero(),
            is_god_node: flag(&row[3]),
        }];
        let mut edges: Vec<EgoEdge> = Vec::new();
        let mut frontier = vec![centre_id.clone()];
        let hop = |table: &str, outbound: bool| {
            let (me, other) = if outbound {
                ("src", "dst")
            } else {
                ("dst", "src")
            };
            format!(
                "SELECT {NEIGHBOUR_ENTITY} FROM \"{table}\" r JOIN Entity b ON b.id = r.{other} \
                 WHERE r.{me} = $id"
            )
        };
        let steps = [
            (hop("RELATES_TO", true), "relates_to", true),
            (hop("RELATES_TO", false), "relates_to", false),
            (hop("ENTITY_CALLS", true), "entity_calls", true),
            (hop("ENTITY_CALLS", false), "entity_calls", false),
        ];
        for _ in 0..depth {
            let mut next = Vec::new();
            for id in &frontier {
                for (sql, type_, outbound) in &steps {
                    for r in self.q(sql, &[("id", text(id))])? {
                        let nid = s(&r[0]);
                        if nid.is_empty() {
                            continue;
                        }
                        let (source, target) = if *outbound {
                            (id.clone(), nid.clone())
                        } else {
                            (nid.clone(), id.clone())
                        };
                        edges.push(EgoEdge {
                            source,
                            target,
                            type_: (*type_).to_string(),
                        });
                        if visited_entities.insert(nid.clone()) {
                            next.push(nid.clone());
                            nodes.push(EgoNode::Entity {
                                id: nid,
                                display: s(&r[1]),
                                mention_count: r[2].as_u64_or_zero(),
                                is_god_node: flag(&r[3]),
                            });
                        }
                    }
                }
                for r in self.q(
                    "SELECT d.id, d.title, d.path FROM \"COVERS\" c JOIN Doc d ON d.id = c.src \
                     WHERE c.dst = $id",
                    &[("id", text(id))],
                )? {
                    let doc_id = s(&r[0]);
                    if doc_id.is_empty() {
                        continue;
                    }
                    edges.push(EgoEdge {
                        source: doc_id.clone(),
                        target: id.clone(),
                        type_: "covers".to_string(),
                    });
                    if docs.insert(doc_id.clone()) {
                        nodes.push(EgoNode::Doc {
                            id: doc_id,
                            title: s(&r[1]),
                            path: s(&r[2]),
                        });
                    }
                }
                for r in self.q(
                    "SELECT f.symbol, f.file, f.line, f.language FROM \"FUNCTION_BELONGS_TO\" b \
                     JOIN Function f ON f.symbol = b.src WHERE b.dst = $id",
                    &[("id", text(id))],
                )? {
                    let symbol = s(&r[0]);
                    if symbol.is_empty() {
                        continue;
                    }
                    edges.push(EgoEdge {
                        source: symbol.clone(),
                        target: id.clone(),
                        type_: "function_belongs_to".to_string(),
                    });
                    if functions.insert(symbol.clone()) {
                        nodes.push(EgoNode::Function {
                            symbol,
                            file: s(&r[1]),
                            line: u32v(&r[2]),
                            language: s(&r[3]),
                        });
                    }
                }
                for r in self.q(
                    "SELECT ep.id, ep.kind, ep.method, ep.path FROM \"ENDPOINT_TOUCHES_ENTITY\" t \
                     JOIN Endpoint ep ON ep.id = t.src WHERE t.dst = $id",
                    &[("id", text(id))],
                )? {
                    let ep_id = s(&r[0]);
                    if ep_id.is_empty() {
                        continue;
                    }
                    edges.push(EgoEdge {
                        source: ep_id.clone(),
                        target: id.clone(),
                        type_: "endpoint_touches_entity".to_string(),
                    });
                    if endpoints.insert(ep_id.clone()) {
                        nodes.push(EgoNode::Endpoint {
                            id: ep_id,
                            endpoint_kind: s(&r[1]),
                            method: s(&r[2]),
                            path: s(&r[3]),
                        });
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        nodes.sort_by_key(|n| {
            let (k, id) = match n {
                EgoNode::Entity { id, .. } => (0u8, id),
                EgoNode::Doc { id, .. } => (1, id),
                EgoNode::Function { symbol, .. } => (2, symbol),
                EgoNode::Endpoint { id, .. } => (3, id),
            };
            (k, id.clone())
        });
        edges.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then(a.target.cmp(&b.target))
                .then(a.type_.cmp(&b.type_))
        });
        edges.dedup_by(|a, b| a.source == b.source && a.target == b.target && a.type_ == b.type_);
        Ok(Some(EntityEgoGraph {
            centre: centre_id,
            depth,
            nodes,
            edges,
        }))
    }

    fn list_endpoints(
        &self,
        kind_filter: Option<&str>,
        dark_only: bool,
    ) -> Result<Vec<EndpointRow>> {
        let mut touches: HashMap<String, Vec<String>> = HashMap::new();
        for r in self.q(
            "SELECT t.src, t.dst FROM \"ENDPOINT_TOUCHES_ENTITY\" t \
             JOIN Entity e ON e.id = t.dst ORDER BY t.src, t.dst",
            &[],
        )? {
            touches.entry(s(&r[0])).or_default().push(s(&r[1]));
        }
        let mut out = Vec::new();
        for r in self.q(
            "SELECT id, kind, method, path, handler_symbol, source_file, source_line \
             FROM Endpoint ORDER BY id",
            &[],
        )? {
            let kind_raw = s(&r[1]);
            if kind_filter.is_some_and(|want| kind_raw != want) {
                continue;
            }
            let Some(kind) = EndpointKind::parse(&kind_raw) else {
                continue;
            };
            let id = s(&r[0]);
            let entities = touches.remove(&id).unwrap_or_default();
            if dark_only && !entities.is_empty() {
                continue;
            }
            out.push(EndpointRow {
                kind,
                method: s(&r[2]),
                path: s(&r[3]),
                handler_symbol: so(&r[4]),
                entities,
                source_file: s(&r[5]),
                source_line: u32v(&r[6]),
                id,
            });
        }
        Ok(out)
    }

    fn endpoint_reach_by_kind(&self) -> Result<Vec<(EndpointKind, u64, u64)>> {
        const KINDS: &[EndpointKind] = &[
            EndpointKind::Axum,
            EndpointKind::Clap,
            EndpointKind::Mcp,
            EndpointKind::Fastapi,
            EndpointKind::Flask,
            EndpointKind::Express,
            EndpointKind::Jaxrs,
        ];
        let count = |sql: &str| -> Result<BTreeMap<EndpointKind, u64>> {
            Ok(self
                .q(sql, &[])?
                .iter()
                .filter_map(|r| Some((EndpointKind::parse(r[0].as_str()?)?, r[1].as_u64_or_zero())))
                .collect())
        };
        let totals = count("SELECT kind, count(*) FROM Endpoint GROUP BY kind")?;
        let reach = count(
            "SELECT e.kind, count(*) FROM Endpoint e WHERE EXISTS \
             (SELECT 1 FROM \"ENDPOINT_TOUCHES_ENTITY\" t JOIN Entity n ON n.id = t.dst \
              WHERE t.src = e.id) GROUP BY e.kind",
        )?;
        Ok(KINDS
            .iter()
            .map(|k| {
                (
                    *k,
                    totals.get(k).copied().unwrap_or(0),
                    reach.get(k).copied().unwrap_or(0),
                )
            })
            .collect())
    }

    fn functions_mentioning(&self, entity_id: &EntityId) -> Result<Vec<FunctionRow>> {
        Ok(self
            .q(
                "SELECT f.symbol, f.crate, f.file, f.line FROM \"FUNCTION_MENTIONS\" m \
                 JOIN Function f ON f.symbol = m.src WHERE m.dst = $ent ORDER BY f.symbol",
                &[("ent", text(entity_id.as_str()))],
            )?
            .iter()
            .map(|r| FunctionRow {
                symbol: s(&r[0]),
                crate_name: s(&r[1]),
                file: s(&r[2]),
                line: u32v(&r[3]),
            })
            .collect())
    }

    fn function_context(&self, symbol: &FunctionSymbol) -> Result<Option<FunctionContext>> {
        let sym = [("sym", text(symbol.as_str()))];
        let Some(function) = self
            .function_full_rows(
                "SELECT symbol, crate, file, line, doc_comment FROM Function WHERE symbol = $sym",
                &sym,
            )?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let mut links: Vec<FunctionNeighbour> = Vec::new();
        for (kind, edge, sql) in [
            (
                "doc",
                "function-defined-in",
                "SELECT d.id, d.title FROM \"FUNCTION_DEFINED_IN\" r JOIN Doc d ON d.id = r.dst \
                 WHERE r.src = $sym",
            ),
            (
                "entity",
                "function-mentions",
                "SELECT e.id, e.display FROM \"FUNCTION_MENTIONS\" r JOIN Entity e ON e.id = r.dst \
                 WHERE r.src = $sym ORDER BY e.id",
            ),
            (
                "type",
                "method-of",
                "SELECT t.symbol, t.kind FROM \"METHOD_OF\" r JOIN Type t ON t.symbol = r.dst \
                 WHERE r.src = $sym ORDER BY t.symbol",
            ),
            (
                "type",
                "uses-type",
                "SELECT t.symbol, t.kind FROM \"USES_TYPE\" r JOIN Type t ON t.symbol = r.dst \
                 WHERE r.src = $sym ORDER BY t.symbol",
            ),
        ] {
            for r in self.q(sql, &sym)? {
                links.push(FunctionNeighbour {
                    kind: kind.to_string(),
                    id: s(&r[0]),
                    title: s(&r[1]),
                    edge: edge.to_string(),
                });
            }
        }
        links.sort_by(|a, b| a.edge.cmp(&b.edge).then_with(|| a.id.cmp(&b.id)));
        Ok(Some(FunctionContext { function, links }))
    }

    fn search_functions_by_symbol_substring(
        &self,
        needle: &str,
        limit: usize,
    ) -> Result<Vec<FunctionFull>> {
        let mut rows = self.function_full_rows(
            "SELECT symbol, crate, file, line, doc_comment FROM Function \
             WHERE instr(symbol, $needle) > 0 ORDER BY symbol",
            &[("needle", text(needle))],
        )?;
        rows.truncate(limit);
        Ok(rows)
    }

    fn function_mentions(&self, symbol: &FunctionSymbol) -> Result<Vec<FunctionMentionRow>> {
        Ok(self
            .q(
                "SELECT e.id, e.display, r.confidence FROM \"FUNCTION_MENTIONS\" r \
                 JOIN Entity e ON e.id = r.dst WHERE r.src = $sym ORDER BY e.id",
                &[("sym", text(symbol.as_str()))],
            )?
            .iter()
            .map(|r| FunctionMentionRow {
                entity_id: s(&r[0]),
                display: s(&r[1]),
                confidence: ConfidenceTier::parse(&s(&r[2])).unwrap_or(ConfidenceTier::High),
            })
            .collect())
    }

    fn docs_covering_entities(&self, entity_ids: &[EntityId]) -> Result<Vec<CoveringDocRow>> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for ent in entity_ids {
            for r in self.q(
                "SELECT d.id, d.title, d.kind FROM \"COVERS\" c JOIN Doc d ON d.id = c.src \
                 WHERE c.dst = $ent ORDER BY d.id",
                &[("ent", text(ent.as_str()))],
            )? {
                let id = s(&r[0]);
                if id.is_empty() || !seen.insert(id.clone()) {
                    continue;
                }
                out.push(CoveringDocRow {
                    id,
                    title: s(&r[1]),
                    kind: so(&r[2]),
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn function_siblings(&self, target: &FunctionSymbol, limit: usize) -> Result<Vec<SiblingRow>> {
        let sym = text(target.as_str());
        let Some(crate_name) = self
            .q(
                "SELECT crate FROM Function WHERE symbol = $sym",
                &[("sym", sym.clone())],
            )?
            .first()
            .map(|r| s(&r[0]))
        else {
            return Ok(Vec::new());
        };
        let mut shared: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for r in self.q(
            "SELECT o.symbol, e.id FROM \"FUNCTION_MENTIONS\" a \
             JOIN Entity e ON e.id = a.dst \
             JOIN \"FUNCTION_MENTIONS\" b ON b.dst = e.id \
             JOIN Function o ON o.symbol = b.src \
             WHERE a.src = $sym AND o.symbol <> $sym AND o.crate = $crate",
            &[("sym", sym), ("crate", text(&crate_name))],
        )? {
            let symbol = s(&r[0]);
            if !symbol.is_empty() {
                shared.entry(symbol).or_default().insert(s(&r[1]));
            }
        }
        let mut rows: Vec<SiblingRow> = shared
            .into_iter()
            .map(|(symbol, ids)| SiblingRow {
                symbol,
                shared_entities: ids.into_iter().collect(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.shared_entities
                .len()
                .cmp(&a.shared_entities.len())
                .then_with(|| a.symbol.cmp(&b.symbol))
        });
        rows.truncate(limit);
        Ok(rows)
    }

    fn query_at(&self, file: &str, line: u32) -> Result<AtQueryResult> {
        let mut out = AtQueryResult {
            function: None,
            file: None,
            module: None,
            entities: Vec::new(),
            covering_docs: Vec::new(),
            nearby_endpoints: Vec::new(),
        };
        let f = [("file", text(file))];
        out.function = self
            .function_full_rows(
                "SELECT symbol, crate, file, line, doc_comment FROM Function \
                 WHERE file = $file AND line <= $line ORDER BY line DESC LIMIT 1",
                &[("file", text(file)), ("line", Value::Int(i64::from(line)))],
            )?
            .into_iter()
            .next();
        if let Some(r) = self
            .q(
                "SELECT path, language, loc, last_touched FROM File WHERE path = $file",
                &f,
            )?
            .first()
        {
            out.file = Some(FileRow {
                path: s(&r[0]),
                language: s(&r[1]),
                loc: u32v(&r[2]),
                last_touched: s(&r[3]),
            });
            if let Some(m) = self
                .q(
                    "SELECT m.id, m.kind, m.path, m.name FROM \"IN_MODULE\" r \
                     JOIN Module m ON m.id = r.dst WHERE r.src = $file LIMIT 1",
                    &f,
                )?
                .first()
            {
                out.module = Some(ModuleRow {
                    id: s(&m[0]),
                    kind: s(&m[1]),
                    path: s(&m[2]),
                    name: s(&m[3]),
                });
            }
        }
        let Some(sym) = out.function.as_ref().map(|f| f.symbol.clone()) else {
            return Ok(out);
        };
        let sp = [("sym", text(&sym))];
        for (edge, table) in [
            ("function-belongs-to", "FUNCTION_BELONGS_TO"),
            ("function-mentions", "FUNCTION_MENTIONS"),
        ] {
            for r in self.q(
                &format!(
                    "SELECT e.id, e.display FROM \"{table}\" r JOIN Entity e ON e.id = r.dst \
                     WHERE r.src = $sym"
                ),
                &sp,
            )? {
                out.entities.push(AtEntityLink {
                    id: s(&r[0]),
                    display: s(&r[1]),
                    edge: edge.to_string(),
                });
            }
        }
        out.entities
            .sort_by(|a, b| a.edge.cmp(&b.edge).then_with(|| a.id.cmp(&b.id)));
        for r in self.q(
            "SELECT DISTINCT d.id, d.title FROM \"COVERS\" c JOIN Doc d ON d.id = c.src \
             WHERE c.dst IN (SELECT dst FROM \"FUNCTION_MENTIONS\" WHERE src = $sym \
                             UNION SELECT dst FROM \"FUNCTION_BELONGS_TO\" WHERE src = $sym) \
             ORDER BY d.id",
            &sp,
        )? {
            out.covering_docs.push(AtCoveringDoc {
                id: s(&r[0]),
                title: s(&r[1]),
            });
        }
        for r in self.q(
            "SELECT ep.id, ep.kind, ep.method, ep.path FROM \"ENDPOINT_HANDLED_BY\" h \
             JOIN Endpoint ep ON ep.id = h.src WHERE h.dst = $sym ORDER BY ep.id",
            &sp,
        )? {
            out.nearby_endpoints.push(AtEndpoint {
                id: s(&r[0]),
                kind: s(&r[1]),
                method: s(&r[2]),
                path: s(&r[3]),
            });
        }
        Ok(out)
    }

    /// Callers within `depth` hops via a recursive CTE over `CALLS` (the
    /// depth cap also terminates call cycles); a caller's level is its
    /// shortest distance to the target.
    fn query_impact(&self, symbol: &str, depth: u32) -> Result<Option<ImpactResult>> {
        const FN: &str = "SELECT symbol, crate, file, line, doc_comment FROM Function";
        let Some(target) = self
            .function_full_rows(
                &format!("{FN} WHERE symbol = $sym"),
                &[("sym", text(symbol))],
            )?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let reach = self.q(
            "WITH RECURSIVE r(sym, d) AS ( \
               SELECT $sym, 0 \
               UNION SELECT c.src, r.d + 1 FROM \"CALLS\" c JOIN r ON c.dst = r.sym \
               WHERE r.d < $depth) \
             SELECT f.symbol, f.crate, f.file, f.line, f.doc_comment, min(r.d) \
             FROM r JOIN Function f ON f.symbol = r.sym WHERE r.d > 0 GROUP BY f.symbol",
            &[
                ("sym", text(symbol)),
                ("depth", Value::Int(i64::from(depth))),
            ],
        )?;
        let mut by_depth: BTreeMap<u32, Vec<FunctionFull>> = BTreeMap::new();
        let mut chain: Vec<String> = vec![target.symbol.clone()];
        for r in &reach {
            by_depth.entry(u32v(&r[5])).or_default().push(FunctionFull {
                symbol: s(&r[0]),
                crate_name: s(&r[1]),
                file: s(&r[2]),
                line: u32v(&r[3]),
                doc_comment: s(&r[4]),
            });
            chain.push(s(&r[0]));
        }
        let depths: Vec<ImpactDepthLevel> = by_depth
            .into_iter()
            .map(|(depth, mut callers)| {
                callers.sort_by(|a, b| a.symbol.cmp(&b.symbol));
                ImpactDepthLevel { depth, callers }
            })
            .collect();

        let mut seen_e: HashSet<String> = HashSet::new();
        let mut entities: Vec<ImpactEntity> = Vec::new();
        let mut seen_p: HashSet<String> = HashSet::new();
        let mut endpoints: Vec<ImpactEndpoint> = Vec::new();
        for sym in &chain {
            let sp = [("sym", text(sym))];
            for r in self.q(
                "SELECT DISTINCT e.id, e.display, e.is_god_node FROM ( \
                   SELECT dst FROM \"FUNCTION_MENTIONS\" WHERE src = $sym \
                   UNION SELECT dst FROM \"FUNCTION_BELONGS_TO\" WHERE src = $sym) x \
                 JOIN Entity e ON e.id = x.dst",
                &sp,
            )? {
                let id = s(&r[0]);
                if !id.is_empty() && seen_e.insert(id.clone()) {
                    entities.push(ImpactEntity {
                        id,
                        display: s(&r[1]),
                        god_node: flag(&r[2]),
                    });
                }
            }
            for r in self.q(
                "SELECT ep.id, ep.kind, ep.method, ep.path FROM \"ENDPOINT_HANDLED_BY\" h \
                 JOIN Endpoint ep ON ep.id = h.src WHERE h.dst = $sym",
                &sp,
            )? {
                let id = s(&r[0]);
                if !id.is_empty() && seen_p.insert(id.clone()) {
                    endpoints.push(ImpactEndpoint {
                        id,
                        kind: s(&r[1]),
                        method: s(&r[2]),
                        path: s(&r[3]),
                    });
                }
            }
        }
        entities.sort_by(|a, b| b.god_node.cmp(&a.god_node).then(a.id.cmp(&b.id)));
        endpoints.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Some(ImpactResult {
            target,
            depths,
            entities,
            endpoints,
            tests: Vec::new(),
        }))
    }

    fn list_dead_code(&self) -> Result<Vec<DeadCodeRow>> {
        Ok(self
            .q(
                "SELECT f.symbol, f.crate, f.file, f.line, f.doc_comment, e.id \
                 FROM Function f \
                 LEFT JOIN \"FUNCTION_BELONGS_TO\" b ON b.src = f.symbol \
                 LEFT JOIN Entity e ON e.id = b.dst \
                 WHERE coalesce(f.generated, 0) = 0 \
                   AND NOT EXISTS (SELECT 1 FROM \"CALLS\" c WHERE c.dst = f.symbol) \
                   AND NOT EXISTS (SELECT 1 FROM \"ENDPOINT_HANDLED_BY\" h WHERE h.dst = f.symbol) \
                 ORDER BY f.file, f.line",
                &[],
            )?
            .iter()
            .filter(|r| !s(&r[0]).is_empty())
            .map(|r| DeadCodeRow {
                symbol: s(&r[0]),
                crate_name: s(&r[1]),
                file: s(&r[2]),
                line: u32v(&r[3]),
                doc_summary: crate::store::query::dead_code::doc_summary(&s(&r[4])),
                entity: s(&r[5]),
            })
            .collect())
    }

    fn list_dark_public_functions(
        &self,
        anchor_required_in: &[String],
        config: &LintConfig,
    ) -> Result<Vec<DarkFunctionRow>> {
        if anchor_required_in.is_empty() {
            return Ok(Vec::new());
        }
        let mut allowed: HashSet<String> = HashSet::new();
        for entry in anchor_required_in {
            if let Some(stripped) = entry.strip_prefix("crates/") {
                let first = stripped.split('/').next().unwrap_or(stripped);
                if !first.is_empty() {
                    allowed.insert(first.to_string());
                }
            } else {
                allowed.insert(entry.clone());
            }
        }
        let rows = self.dark_rows(&format!("{DARK_FN} ORDER BY f.crate, f.symbol"), &[])?;
        Ok(rows
            .into_iter()
            .filter(|r| {
                allowed.contains(&r.crate_name)
                    || anchor_required_in
                        .iter()
                        .any(|p| !p.is_empty() && r.file.starts_with(p))
            })
            .filter(|r| {
                crate::scaffold::bare_fn_name(&r.symbol)
                    .is_none_or(|bare| !config.is_function_exempt(&bare))
            })
            .collect())
    }

    fn list_all_dark_public_functions(&self, config: &LintConfig) -> Result<Vec<DarkFunctionRow>> {
        let rows = self.dark_rows(&format!("{DARK_FN} ORDER BY f.crate, f.symbol"), &[])?;
        Ok(rows
            .into_iter()
            .filter(|r| {
                let Some(bare) = crate::scaffold::bare_fn_name(&r.symbol) else {
                    return true;
                };
                !config
                    .coverage
                    .coverage_function_exempt_set
                    .as_ref()
                    .is_some_and(|rxs| rxs.iter().any(|x| x.is_match(&bare)))
            })
            .collect())
    }

    fn list_dark_endpoints(&self) -> Result<Vec<DarkEndpointRow>> {
        Ok(self
            .q(
                "SELECT e.kind, e.method, e.path, e.source_file, e.source_line FROM Endpoint e \
                 WHERE NOT EXISTS (SELECT 1 FROM \"ENDPOINT_TOUCHES_ENTITY\" t \
                                   JOIN Entity n ON n.id = t.dst WHERE t.src = e.id) \
                 ORDER BY e.kind, e.path",
                &[],
            )?
            .iter()
            .filter_map(|r| {
                Some(DarkEndpointRow {
                    kind: EndpointKind::parse(r[0].as_str()?)?,
                    method: s(&r[1]),
                    path: s(&r[2]),
                    file: s(&r[3]),
                    line: u32v(&r[4]),
                })
            })
            .collect())
    }

    fn list_entity_coverage_gaps(
        &self,
        threshold: f32,
        exempt: &[String],
    ) -> Result<Vec<EntityGapRow>> {
        if threshold <= 0.0 {
            return Ok(Vec::new());
        }
        let doc_counts = typed::entity_doc_counts(self)?;
        let func_counts = typed::entity_func_counts(self)?;
        let exempt: HashSet<&str> = exempt.iter().map(String::as_str).collect();
        let all: BTreeSet<&String> = doc_counts.keys().chain(func_counts.keys()).collect();
        let mut out = Vec::new();
        for ent in all {
            if exempt.contains(ent.as_str()) {
                continue;
            }
            let dc = doc_counts.get(ent).copied().unwrap_or(0);
            let fc = func_counts.get(ent).copied().unwrap_or(0);
            if fc == 0 {
                continue;
            }
            let ratio = dc as f32 / fc as f32;
            if ratio < threshold {
                out.push(EntityGapRow {
                    entity: ent.clone(),
                    doc_count: dc,
                    func_count: fc,
                    ratio,
                });
            }
        }
        Ok(out)
    }

    fn global_function_reach(&self, config: &LintConfig) -> Result<(u64, u64, u64)> {
        let total = typed::count_functions(self)?;
        let reach = typed::count_functions_reaching_entity(self)?;
        let (exempt, _) = crate::coverage::exempt_dark_breakdown(self, config)?;
        Ok((total, reach, exempt))
    }

    fn list_dark_functions_in_crate(&self, crate_name: &str) -> Result<Vec<DarkFunctionRow>> {
        self.dark_rows(
            &format!("{DARK_FN} AND f.crate = $crate ORDER BY f.file, f.line, f.symbol"),
            &[("crate", text(crate_name))],
        )
    }

    fn run_saved_query(
        &self,
        root: Option<&Path>,
        name: &str,
        params: &HashMap<String, String>,
    ) -> Result<serde_json::Value> {
        super::saved::run_saved_query_with_root(self, root, name, params)
    }

    fn run_sql(&self, sql: &str, params: Vec<(&str, Value)>) -> Result<serde_json::Value> {
        super::saved::run_readonly_sql(self, sql, &params)
    }

    fn schema_tables(&self) -> Result<Vec<crate::graph_read::TableInfo>> {
        super::schema::schema_tables(self)
    }

    fn embedding_nearest(
        &self,
        req: &crate::graph_read::NearestRequest<'_>,
    ) -> Result<crate::graph_read::NearestResult> {
        super::nearest::nearest(self, req)
    }
}
