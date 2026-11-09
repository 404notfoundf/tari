# 上游 Release 报告：{{tag}}

## 1. 范围

对比分支：`{{branch}}`

官方 tag：`{{tag}}`

Release：{{url}}

{{status}}
详细材料覆盖节点、钱包、base_layer/core、gRPC 和对应 client。merge-mining proxy 不是审查重点。
这份报告不 merge，也不 push。

## 2. 处理建议

{{decision}}

本机切到 `{{branch}}` 后执行 `/merge-upstream {{tag}}`。
skill 会新建 `feat/merge-<tag>`，结束时把 origin 改回 fork。不 push，不 cargo build。
Class A 冲突逐段问人，禁止自动选边。版本号、`Cargo.lock`、生成物属于 Class B，自动采用官方。其它源码冲突属于 Class C，问人。
文件重叠不是已经发生的 Git 冲突。预设名单以外的文件仍可能冲突。

## 3. 分析

{{analysis}}

## 4. 重叠文件

Class A，双方都改过。冲突时逐段问人，不自动选边：

{{class_a_overlap}}

Class C，双方都改过。其它源码冲突时问人：

{{class_c_overlap}}

只有官方改过。这些预设文件 merge 时采用官方改动：

{{only_upstream}}

Class B 不在预设名单里。真正 merge 时，版本号、`Cargo.lock`、生成物才会自动采用官方。
{{footer}}
<!-- appendix -->
## 附录

这份附录是完整的，没有按 Issue 字数截断。关键词命中不是升级结论。

### 提交说明关键词

{{important}}

### 可疑新增行

```text
{{suspicious}}
```

### 发行说明

{{notes}}

### changelog

```markdown
{{changelog}}
```

### 节点和钱包改动统计

```text
{{focus_stat}}
```

### 全仓库改动统计

```text
{{stat}}
```

### 预设重点文件：上游一侧

```diff
{{upstream_diff}}
```

### 预设重点文件：Fork 一侧

```diff
{{fork_diff}}
```

### 本地复核

```bash
git fetch https://github.com/{{upstream_repo}}.git tag {{tag}}
git diff {{branch}}...{{tag}}
```
