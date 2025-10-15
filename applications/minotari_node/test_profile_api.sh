#!/bin/bash

# 测试Profile HTTP API的脚本
# 使用方法: ./test_profile_api.sh [base_url]

BASE_URL=${1:-"http://localhost:8080"}

echo "测试Profile HTTP API..."
echo "基础URL: $BASE_URL"
echo ""

# 测试获取profiling状态
echo "1. 测试获取profiling状态..."
curl -s -X GET "$BASE_URL/profile/status" | jq '.' 2>/dev/null || curl -s -X GET "$BASE_URL/profile/status"
echo ""
echo ""

# 测试触发内存分析（不包含内容）
echo "2. 测试触发内存分析（不包含内容）..."
curl -s -X GET "$BASE_URL/profile/memory" | jq '.' 2>/dev/null || curl -s -X GET "$BASE_URL/profile/memory"
echo ""
echo ""

# 测试触发内存分析（包含内容）
echo "3. 测试触发内存分析（包含内容）..."
curl -s -X GET "$BASE_URL/profile/memory?include_content=true" | jq '.' 2>/dev/null || curl -s -X GET "$BASE_URL/profile/memory?include_content=true"
echo ""
echo ""

# 测试Swagger UI
echo "4. 访问Swagger UI: $BASE_URL/swagger-ui"
echo ""

echo "测试完成！"
