#!/bin/bash
# L6 reasoning-rules drill: boot an isolated gateway (memory storage, enforced
# auth, inference off by default), assert the complete 87-rule forward-chaining
# set is advertised, then verify representative newly-added rules actually fire
# end-to-end over SPARQL (?inference=forward) and that per-request inference is
# off by default. Self-contained; never touches production data.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/ontolith-server}
PORT=${ONTOLITH_DRILL_PORT:-18083}
GRPC_PORT=${ONTOLITH_DRILL_GRPC_PORT:-15083}
API_KEY="drill-key-0123456789abcdef0123456789abcdef"

WORK=$(mktemp -d)
trap 'kill "$PID" 2>/dev/null || true; rm -rf "$WORK"' EXIT

export ONTOLITH_BIND="127.0.0.1:$PORT"
export ONTOLITH_GRPC_BIND="127.0.0.1:$GRPC_PORT"
export ONTOLITH_AUTH_MODE=enforced
export ONTOLITH_API_KEY="$API_KEY"
export ONTOLITH_TENANT_MODE=enforced
export ONTOLITH_INFERENCE_MODE=off

"$BIN" > "$WORK/server.log" 2>&1 &
PID=$!
sleep 2

BASE="http://127.0.0.1:$PORT"
H=(-H "x-api-key: $API_KEY" -H "x-ontolith-tenant: default" -H "x-ontolith-user: drill")

pass=0; fail=0
check() { # name expected actual
  if [ "$2" = "$3" ]; then pass=$((pass+1)); echo "PASS: $1"; else fail=$((fail+1)); echo "FAIL: $1 (expected=$2 got=$3)"; fi
}
json() { python3 -c "import json,sys; d=json.load(sys.stdin); $1"; }
sel_has() { # file var value
  python3 -c "
import json,sys
d=json.load(open('$1'))
vals=[b['$2']['value'] for b in d.get('results',{}).get('bindings',[]) if '$2' in b]
sys.exit(0 if '$3' in vals else 1)
"
}

echo "== rules advertised =="
curl -s --max-time 5 "${H[@]}" "$BASE/inference" -o "$WORK/inference.json"
check "inference http 200" 200 "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "${H[@]}" "$BASE/inference")"
N=$(python3 -c "import json; print(len(json.load(open('$WORK/inference.json'))['rules']))")
check "rule list count == 87" 87 "$N"
for r in eq-ref prp-eqp1 prp-asyp prp-key cls-maxc1 cls-maxqc3 cls-oo cax-eqc1 scm-sco scm-avf2 dt-not-type rdfs12; do
  json "print(1 if '$r' in d['rules'] else 0)" < "$WORK/inference.json" > "$WORK/one"
  check "rule $r present" 1 "$(cat "$WORK/one")"
done

echo "== control: inference off does not fire rules =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql" <<'Q' > "$WORK/ctrl.json"
PREFIX : <urn:ex:> ASK { :alice :knowsAlias :bob }
Q
check "inference=off :alice knowsAlias bob" false "$(json "print(str(d['boolean']).lower())" < "$WORK/ctrl.json")"

echo "== ingest ontology exercising new rules =="
CODE=$(curl -s -o "$WORK/ingest.out" -w '%{http_code}' --max-time 10 "${H[@]}" \
  -H 'content-type: text/turtle' --data-binary @- "$BASE/data/turtle" <<'TTL'
@prefix : <urn:ex:> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

:knows owl:equivalentProperty :knowsAlias .
:alice :knows :bob .

:worksAt rdfs:domain :Employee .
:Employee rdfs:subClassOf :Person .
:carol :worksAt :acme .

:reportsTo rdfs:range :Employee .
:dave :reportsTo :eve .

:petRestr owl:onProperty :hasPet ; owl:maxQualifiedCardinality 1 ; owl:onClass :Cat .
:thingRestr owl:onProperty :hasPet ; owl:maxQualifiedCardinality 1 ; owl:onClass owl:Thing .
:cat1 rdf:type :Cat .
:cat2 rdf:type :Cat .
:frank rdf:type :petRestr ; rdf:type :thingRestr .
:frank :hasPet :cat1 ; :hasPet :cat2 ; :hasPet :cat3 ; :hasPet :cat4 .

:Person owl:hasKey ( :ssn ) .
:grace rdf:type :Person ; :ssn "S-42" .
:heidi rdf:type :Person ; :ssn "S-42" .
TTL
)
check "ingest turtle 200" 200 "$CODE"
head -c 200 "$WORK/ingest.out"; echo

echo "== prp-eqp1/2 + prp-spo1: equivalent property fires both directions =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q1.json"
PREFIX : <urn:ex:> SELECT ?o WHERE { :alice :knowsAlias ?o }
Q
if sel_has "$WORK/q1.json" o "urn:ex:bob"; then check "prp-eqp1 -> knowsAlias bob" yes yes; else check "prp-eqp1 -> knowsAlias bob" yes no; fi
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q2.json"
PREFIX : <urn:ex:> SELECT ?o WHERE { :alice :knows ?o }
Q
if sel_has "$WORK/q2.json" o "urn:ex:bob"; then check "prp-eqp2 + prp-spo1 -> knows bob" yes yes; else check "prp-eqp2 + prp-spo1 -> knows bob" yes no; fi

echo "== prp-dom + scm-dom1: domain typing and superclass propagation =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q3.json"
PREFIX : <urn:ex:> SELECT ?c WHERE { :carol rdf:type ?c }
Q
if sel_has "$WORK/q3.json" c "urn:ex:Employee" && sel_has "$WORK/q3.json" c "urn:ex:Person"; then
  check "prp-dom + scm-sco/cax-sco -> carol Employee,Person" yes yes
else
  check "prp-dom + scm-sco/cax-sco -> carol Employee,Person" yes no
fi
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q4.json"
PREFIX : <urn:ex:> SELECT ?d WHERE { :worksAt rdfs:domain ?d }
Q
if sel_has "$WORK/q4.json" d "urn:ex:Person"; then check "scm-dom1 -> worksAt domain Person" yes yes; else check "scm-dom1 -> worksAt domain Person" yes no; fi

echo "== prp-rng + scm-rng1: range superclass propagation =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q5.json"
PREFIX : <urn:ex:> SELECT ?r WHERE { :reportsTo rdfs:range ?r }
Q
if sel_has "$WORK/q5.json" r "urn:ex:Person"; then check "scm-rng1 -> reportsTo range Person" yes yes; else check "scm-rng1 -> reportsTo range Person" yes no; fi

echo "== cls-maxqc3/4: max 1 qualified cardinality -> sameAs values =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q6.json"
PREFIX : <urn:ex:> PREFIX owl: <http://www.w3.org/2002/07/owl#> ASK { :cat1 owl:sameAs :cat2 }
Q
check "cls-maxqc3 -> cat1 sameAs cat2" True "$(json "print(d['boolean'])" < "$WORK/q6.json")"
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q7.json"
PREFIX : <urn:ex:> PREFIX owl: <http://www.w3.org/2002/07/owl#> ASK { :cat3 owl:sameAs :cat4 }
Q
check "cls-maxqc4 -> cat3 sameAs cat4" True "$(json "print(d['boolean'])" < "$WORK/q7.json")"

echo "== prp-key + eq-ref: hasKey -> sameAs, reflexive sameAs =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q8.json"
PREFIX : <urn:ex:> PREFIX owl: <http://www.w3.org/2002/07/owl#> ASK { :grace owl:sameAs :heidi }
Q
check "prp-key -> grace sameAs heidi" True "$(json "print(d['boolean'])" < "$WORK/q8.json")"
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q9.json"
PREFIX : <urn:ex:> PREFIX owl: <http://www.w3.org/2002/07/owl#> ASK { :grace owl:sameAs :grace }
Q
check "eq-ref -> grace sameAs grace" True "$(json "print(d['boolean'])" < "$WORK/q9.json")"

echo "== rdfs4a/4b + rdfs1/13: resource and datatype typing =="
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q10.json"
PREFIX : <urn:ex:> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ASK { :cat1 rdf:type rdfs:Resource }
Q
check "rdfs4a -> cat1 Resource" True "$(json "print(d['boolean'])" < "$WORK/q10.json")"
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q11.json"
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ASK { xsd:integer rdf:type rdfs:Datatype }
Q
check "rdfs1 -> integer Datatype" True "$(json "print(d['boolean'])" < "$WORK/q11.json")"
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q13.json"
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ASK { xsd:integer rdfs:subClassOf rdfs:Literal }
Q
check "rdfs13 -> integer subClassOf Literal" True "$(json "print(d['boolean'])" < "$WORK/q13.json")"

echo "== consistency rules: maxCardinality 0 and XSD lexical violation -> inconsistent =="
CODE=$(curl -s -o "$WORK/ingest2.out" -w '%{http_code}' --max-time 10 "${H[@]}" \
  -H 'content-type: text/turtle' --data-binary @- "$BASE/data/turtle" <<'TTL'
@prefix : <urn:ex:> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

:max0Restr owl:onProperty :p ; owl:maxCardinality 0 .
:z rdf:type :max0Restr .
:z :p :v .
:lit :value "abc"^^xsd:integer .
TTL
)
check "ingest inconsistent 200" 200 "$CODE"
curl -s --max-time 5 "${H[@]}" --data-binary @- "$BASE/sparql?inference=forward" <<'Q' > "$WORK/q12.json"
PREFIX : <urn:ex:> ASK { :z rdf:type :max0Restr }
Q
check "inconsistent flagged (cls-maxc1 + dt-not-type)" True "$(json "print(d['meta']['reasoning']['inconsistent'])" < "$WORK/q12.json")"

echo
echo "=== REASONING-RULES DRILL: pass=$pass fail=$fail ==="
[ "$fail" -eq 0 ] || { echo "DRILL FAILED"; tail -20 "$WORK/server.log" >&2 || true; exit 1; }
echo "=== REASONING-RULES DRILL PASS ==="
