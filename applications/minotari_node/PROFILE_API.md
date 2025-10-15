# Profile HTTP API 使用说明

## 概述

Profile HTTP API 提供了一个独立的HTTP服务，用于按需触发内存分析并生成Profile文件。服务运行在端口 **17100**。

## 启动服务

Profile服务器会在minotari_node启动时自动启动。无需额外配置。

## API端点

### 1. 获取Profiling状态
```
GET http://localhost:17100/profile/status
```

**响应示例：**
```json
{
  "success": true,
  "message": "Jemalloc profiling 状态获取成功",
  "profiling_available": true,
  "profiling_active": true,
  "platform_supported": true
}
```

### 2. 触发内存分析
```
GET http://localhost:17100/profile/memory
```

**查询参数：**
- `include_content` (可选, boolean): 是否在响应中包含文件内容，默认为false

**响应示例：**
```json
{
  "success": true,
  "message": "内存分析完成，文件已保存: memory_profile_20241201_143022.pb",
  "filename": "memory_profile_20241201_143022.pb",
  "content": null,
  "size": 1024
}
```

### 3. API文档
```
GET http://localhost:17100/swagger-ui
```

## 测试脚本

### Windows
```bash
test_profile_api.bat
```

### Linux/Mac
```bash
curl -X GET http://localhost:17100/profile/status
curl -X GET http://localhost:17100/profile/memory
curl -X GET http://localhost:17100/profile/memory?include_content=true
```

## 生成的文件

- 文件名格式：`memory_profile_YYYYMMDD_HHMMSS.pb`
- 文件位置：当前工作目录
- 文件格式：pprof protobuf格式
- 用途：可用于内存性能分析和火焰图生成

## 注意事项

1. **平台支持**：仅在非MSVC平台（Linux/macOS）上支持jemalloc profiling
2. **性能影响**：内存分析会消耗一定系统资源，建议在低负载时使用
3. **文件大小**：生成的profile文件可能较大，注意磁盘空间
4. **权限要求**：需要写入当前目录的权限

## 故障排除

### 服务未启动
- 检查minotari_node是否正常运行
- 检查端口17100是否被占用
- 查看日志中的Profile服务器启动信息

### Profiling不可用
- 确认运行在非MSVC平台
- 检查jemalloc是否已正确配置
- 查看状态API的响应信息

### 文件生成失败
- 检查磁盘空间
- 确认当前目录有写入权限
- 查看错误日志获取详细信息
