# 🤖 上游 Release 报告：{{tag}}

> 用来判断要不要在本机执行 `/merge-upstream`。不 merge，不 push，不 cargo build。编译和测试没有执行。

---

## 1. 结论

**{{decision}}**

{{reason}}

本机切到 `{{branch}}` 后执行 `/merge-upstream {{tag}}`。
skill 会新建 `feat/merge-<tag>`，结束时把 origin 改回 fork。
Class A 冲突逐段问人，禁止自动选边。版本号、`Cargo.lock`、生成物属于 Class B，自动采用官方。其它源码冲突属于 Class C，问人。

## 2. 范围

| 项目 | 内容 |
|---|---|
| Fork 分支 | `{{branch}}` |
| 上游 tag | `{{tag}}` |
| Release | {{url}} |

{{status}}

重点详细看节点、钱包、共识和 gRPC。其它目录里的硬分叉、漏洞、破坏性变更、密钥泄露和安全问题也会检查。merge-mining proxy 不展开业务流程。

## 3. 发行说明重点

每条一行。已确认和可能影响在前，待验证在后。

{{highlights}}

## 4. 代码风险和核对

{{analysis}}

## 5. 重叠文件

文件重叠不是已经发生的 Git 冲突。预设名单以外的文件仍可能冲突。

Class A，双方都改过。冲突时逐段问人，不自动选边：

{{class_a_overlap}}

Class C，双方都改过。其它源码冲突时问人：

{{class_c_overlap}}

只有官方改过。这些预设文件 merge 时采用官方改动：

{{only_upstream}}

Class B 不在预设名单里。真正 merge 时，版本号、`Cargo.lock`、生成物才会自动采用官方。
{{footer}}
