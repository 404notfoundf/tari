// Copyright 2025 The Tari Project
// SPDX-License-Identifier: BSD-3-Clause

use axum::{
    extract::Query,
    http::StatusCode,
    response::Json,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

// 直接在处理器中实现内存分析函数
#[cfg(not(target_env = "msvc"))]
async fn dump_memory_profile() -> Result<String, String> {
    // 获取 jemalloc 的 profiling 控制器
    let prof_ctl = jemalloc_pprof::PROF_CTL.as_ref()
        .ok_or_else(|| "Profiling controller not available".to_string())?;

    let mut prof_ctl = prof_ctl.lock().await;
    
    // 检查 profiling 是否已激活
    if !prof_ctl.activated() {
        return Err("Jemalloc profiling is not activated".to_string());
    }
   
    // 调用 dump_pprof() 方法生成 pprof 数据
    let pprof_data = prof_ctl.dump_pprof()
        .map_err(|e| format!("Failed to dump pprof: {}", e))?;

    // 使用时间戳生成唯一文件名
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("memory_profile_{}.pb", timestamp);

    // 将 pprof 数据写入本地文件
    std::fs::write(&filename, pprof_data)
        .map_err(|e| format!("Failed to write profile file: {}", e))?;

    info!("Memory profile dumped to: {}", filename);
    Ok(filename)
}

const LOG_TARGET: &str = "c::bn::rpc::http::profile";

/// 内存分析请求参数
#[derive(Debug, Deserialize, IntoParams)]
pub struct ProfileRequest {
    /// 是否返回文件内容（默认为false，只返回文件名）
    #[serde(default)]
    pub include_content: bool,
}

/// 内存分析响应
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileResponse {
    /// 操作是否成功
    pub success: bool,
    /// 响应消息
    pub message: String,
    /// 生成的文件名（如果成功）
    pub filename: Option<String>,
    /// 文件内容（如果请求包含内容）
    pub content: Option<Vec<u8>>,
    /// 文件大小（字节）
    pub size: Option<u64>,
}

/// 触发内存分析并生成Profile文件
#[utoipa::path(
    get,
    path = "/profile/memory",
    params(
        ProfileRequest
    ),
    responses(
        (status = 200, description = "内存分析成功", body = ProfileResponse),
        (status = 500, description = "内存分析失败", body = ProfileResponse)
    ),
    tag = "Profile"
)]
pub async fn handle_memory_profile(
    Query(params): Query<ProfileRequest>,
) -> Result<Json<ProfileResponse>, StatusCode> {
    info!(target: LOG_TARGET, "收到内存分析请求，包含内容: {}", params.include_content);

    // 调用内存分析函数
    #[cfg(not(target_env = "msvc"))]
    match dump_memory_profile().await {
        Ok(filename) => {
            info!(target: LOG_TARGET, "内存分析完成，文件: {}", filename);
            
            let mut response = ProfileResponse {
                success: true,
                message: format!("内存分析完成，文件已保存: {}", filename),
                filename: Some(filename.clone()),
                content: None,
                size: None,
            };

            // 如果请求包含文件内容
            if params.include_content {
                match std::fs::read(&filename) {
                    Ok(content) => {
                        response.content = Some(content.clone());
                        response.size = Some(content.len() as u64);
                        info!(target: LOG_TARGET, "文件内容已读取，大小: {} 字节", content.len());
                    }
                    Err(e) => {
                        error!(target: LOG_TARGET, "读取文件内容失败: {}", e);
                        response.message = format!("文件生成成功但读取内容失败: {}", e);
                    }
                }
            } else {
                // 即使不返回内容，也获取文件大小
                if let Ok(metadata) = std::fs::metadata(&filename) {
                    response.size = Some(metadata.len());
                }
            }

            Ok(Json(response))
        }
        Err(e) => {
            error!(target: LOG_TARGET, "内存分析失败: {}", e);
            let response = ProfileResponse {
                success: false,
                message: format!("内存分析失败: {}", e),
                filename: None,
                content: None,
                size: None,
            };
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
    
    #[cfg(target_env = "msvc")]
    {
        let response = ProfileResponse {
            success: false,
            message: "当前平台不支持 Jemalloc profiling (MSVC)".to_string(),
            filename: None,
            content: None,
            size: None,
        };
        Err(StatusCode::NOT_IMPLEMENTED)
    }
}

/// 获取内存分析状态
#[utoipa::path(
    get,
    path = "/profile/status",
    responses(
        (status = 200, description = "获取状态成功", body = ProfileStatusResponse)
    ),
    tag = "Profile"
)]
pub async fn handle_profile_status() -> Result<Json<ProfileStatusResponse>, StatusCode> {
    #[cfg(not(target_env = "msvc"))]
    {
        use jemalloc_pprof::PROF_CTL;
        
        if let Some(prof_ctl) = PROF_CTL.as_ref() {
            let prof_ctl = prof_ctl.lock().await;
            let is_activated = prof_ctl.activated();
            
            let response = ProfileStatusResponse {
                success: true,
                message: "Jemalloc profiling 状态获取成功",
                profiling_available: true,
                profiling_active: is_activated,
                platform_supported: true,
            };
            
            Ok(Json(response))
        } else {
            let response = ProfileStatusResponse {
                success: false,
                message: "Jemalloc profiling 控制器不可用".to_string(),
                profiling_available: false,
                profiling_active: false,
                platform_supported: true,
            };
            
            Ok(Json(response))
        }
    }
    
    #[cfg(target_env = "msvc")]
    {
        let response = ProfileStatusResponse {
            success: false,
            message: "当前平台不支持 Jemalloc profiling (MSVC)".to_string(),
            profiling_available: false,
            profiling_active: false,
            platform_supported: false,
        };
        
        Ok(Json(response))
    }
}

/// 内存分析状态响应
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileStatusResponse {
    /// 操作是否成功
    pub success: bool,
    /// 响应消息
    pub message: String,
    /// 是否支持profiling
    pub profiling_available: bool,
    /// profiling是否已激活
    pub profiling_active: bool,
    /// 平台是否支持
    pub platform_supported: bool,
}
