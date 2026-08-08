#!/bin/bash
# 同步 PROGRESS.md 条目到 GitHub Projects #2（用户级 Projects v2，SYNC-PROJ-0001）。
# 用法: scripts/sync-github-projects.sh [items.tsv]
#   items.tsv 每行: 标题<TAB>状态<TAB>[优先级]；状态 ∈ 未开始|进行中|已完成，优先级 ∈ P0|P1|P2（可空）。
# 前置: Classic PAT（project scope）写入 /tmp/gh_token（chmod 600）。幂等：按标题匹配，缺失创建，存在只更新字段。
# 看板/字段 ID（projects/2）：
#   项目 PVT_kwHOAsUzTs4Bfxo4 · Status PVTSSF_lAHOAsUzTs4Bfxo4zhaCHXU（Todo f75ad846 / In progress 47fc9ee4 / Done 98236657）
#   Priority PVTSSF_lAHOAsUzTs4Bfxo4zhaCH3g（P0 b43f28f8 / P1 3cd6b8a4 / P2 aadc3c89）
set -uo pipefail
TSV="${1:-/tmp/sync-items.tsv}"
TOKEN=$(cat /tmp/gh_token)
PROJ=PVT_kwHOAsUzTs4Bfxo4
F_STATUS=PVTSSF_lAHOAsUzTs4Bfxo4zhaCHXU
F_PRIO=PVTSSF_lAHOAsUzTs4Bfxo4zhaCH3g
API=https://api.github.com/graphql
declare -A ST=([未开始]=f75ad846 [进行中]=47fc9ee4 [已完成]=98236657)
declare -A PR=([P0]=b43f28f8 [P1]=3cd6b8a4 [P2]=aadc3c89)
AUTH=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")

gql() { curl -sS --max-time 30 "${AUTH[@]}" "$API" -d "$1"; }

# 现有条目: title -> item id
mapfile -t EXIST < <(gql '{"query":"query{node(id:\"'"$PROJ"'\"){... on ProjectV2{items(first:100){nodes{id content{... on DraftIssue{title}}}}}}}"}' \
  | jq -r '.data.node.items.nodes[] | [.id, .content.title] | @tsv')
declare -A IDS=()
for row in "${EXIST[@]}"; do IDS[${row#*$'\t'}]=${row%%$'\t'*}; done

set_field() { # item field option
  gql "$(jq -n --arg p "$PROJ" --arg i "$1" --arg f "$2" --arg o "$3" \
    '{query:"mutation($p:ID!,$i:ID!,$f:ID!,$o:String!){updateProjectV2ItemFieldValue(input:{projectId:$p,itemId:$i,fieldId:$f,value:{singleSelectOptionId:$o}}){projectV2Item{id}}}",variables:{p:$p,i:$i,f:$f,o:$o}}')" \
    | jq -r 'if .errors then "ERR " + (.errors[0].message) else "ok" end'
}
create_item() { # title -> item id
  gql "$(jq -n --arg p "$PROJ" --arg t "$1" \
    '{query:"mutation($p:ID!,$t:String!){addProjectV2DraftIssue(input:{projectId:$p,title:$t}){projectItem{id}}}",variables:{p:$p,t:$t}}')" \
    | jq -r 'if .errors then "ERR " + (.errors[0].message) else .data.addProjectV2DraftIssue.projectItem.id end'
}

ok=0; upd=0; fail=0
while IFS=$'\t' read -r title status prio; do
  [ -z "${title:-}" ] && continue
  if [[ -n "${IDS[$title]:-}" ]]; then item=${IDS[$title]}; upd=$((upd+1)); else
    item=$(create_item "$title")
    if [[ "$item" == ERR* ]]; then echo "FAIL create: $title -> $item"; fail=$((fail+1)); continue; fi
  fi
  r=$(set_field "$item" "$F_STATUS" "${ST[$status]}")
  if [[ "$r" != ok ]]; then echo "FAIL status: $title -> $r"; fail=$((fail+1)); continue; fi
  if [[ -n "${prio:-}" && -n "${PR[$prio]:-}" ]]; then
    r=$(set_field "$item" "$F_PRIO" "${PR[$prio]}")
    [[ "$r" != ok ]] && echo "WARN prio: $title -> $r"
  fi
  ok=$((ok+1))
done < "$TSV"
echo "SYNC DONE ok=$ok updated=$upd fail=$fail"
