# SPARQL 测试样例（loadtest 10000 条）

## 1 基础计数
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT (COUNT(?s) AS ?total) WHERE { ?s ?p ?o }
```

验证：OK | 389ms | rows=1 (count 1) | 10009

## 2 谓词分布
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?p (COUNT(?s) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)
```

验证：OK | 380ms | rows=9 (count 9) | http://ontolith.example/loadtest/label 3000

## 3 标签模糊检索
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?o WHERE { ?s ex:label ?o . FILTER(CONTAINS(STR(?o), "LoadTest-00025")) } LIMIT 10
```

验证：OK | 492ms | rows=10 (count 10) | http://ontolith.example/loadtest/row-000250 LoadTest-000250 中文标签 250

## 4 整数区间过滤
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?v WHERE { ?s ex:value ?v . FILTER(?v >= 4000 && ?v < 4500) } ORDER BY ?v LIMIT 20
```

验证：OK | 493ms | rows=20 (count 20) | http://ontolith.example/loadtest/row-004000 4000

## 5 小数过滤排序
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?price WHERE { ?s ex:price ?price . FILTER(?price >= 60) } ORDER BY DESC(?price) LIMIT 10
```

验证：OK | 323ms | rows=10 (count 10) | http://ontolith.example/loadtest/row-006999 69.99

## 6 owner 分组统计
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?owner (COUNT(?s) AS ?n) WHERE { ?s ex:owner ?owner } GROUP BY ?owner ORDER BY DESC(?n) LIMIT 10
```

验证：OK | 266ms | rows=10 (count 10) | http://ontolith.example/users/user-0 10

## 7 状态分组
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?st (COUNT(?s) AS ?n) WHERE { ?s ex:status ?st } GROUP BY ?st ORDER BY ?st
```

验证：OK | 282ms | rows=1001 (count 1001) | active-9000 1

## 8 主语前缀过滤
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?st WHERE { ?s ex:status ?st . FILTER(STRSTARTS(STR(?s), "http://ontolith.example/loadtest/row-009")) } ORDER BY ?s LIMIT 10
```

验证：OK | 271ms | rows=10 (count 10) | http://ontolith.example/loadtest/row-009002 err-9002

## 9 最大值排序
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?v WHERE { ?s ex:value ?v } ORDER BY DESC(?v) LIMIT 5
```

验证：OK | 275ms | rows=5 (count 5) | http://ontolith.example/loadtest/row-004999 4999

## 10 UNION 多模式
```sparql
PREFIX ex: <http://ontolith.example/loadtest/> SELECT ?s ?kind WHERE { { ?s ex:label ?o BIND("label" AS ?kind) } UNION { ?s ex:value ?v BIND("value" AS ?kind) } } ORDER BY ?s LIMIT 10
```

验证：OK | 460ms | rows=10 (count 10) | http://ontolith.example/loadtest/row-000000 label
