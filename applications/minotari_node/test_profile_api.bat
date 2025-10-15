@echo off
REM 测试Profile HTTP API的Windows批处理脚本
REM Profile服务运行在端口17100

set BASE_URL=http://localhost:17100

echo ========================================
echo 测试Profile HTTP API
echo ========================================
echo 基础URL: %BASE_URL%
echo 注意：请确保minotari_node已经启动
echo.

REM 测试获取profiling状态
echo 1. 测试获取profiling状态...
curl -s -X GET "%BASE_URL%/profile/status"
echo.
echo.

REM 测试触发内存分析（不包含内容）
echo 2. 测试触发内存分析（不包含内容）...
curl -s -X GET "%BASE_URL%/profile/memory"
echo.
echo.

REM 测试触发内存分析（包含内容）
echo 3. 测试触发内存分析（包含内容）...
curl -s -X GET "%BASE_URL%/profile/memory?include_content=true"
echo.
echo.

REM 测试Swagger UI
echo 4. 访问Swagger UI: %BASE_URL%/swagger-ui
echo.

echo 测试完成！
pause