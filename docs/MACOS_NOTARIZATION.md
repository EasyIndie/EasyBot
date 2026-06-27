# macOS 公证配置指南

EasyBot 发布工作流支持可选 macOS 代码签名 + Apple 公证。启用后，`*-apple-darwin` 二进制会自动签名并通过 Apple 公证，用户下载后不会触发 Gatekeeper 安全警告。

## 前置条件

- **Apple Developer Program** 会员（$99/年）
- 一个已激活的 Apple Developer 账号
- 本仓库的 **Admin** 权限（用于添加 GitHub Secrets）

## 步骤

### 1. 创建 Developer ID Application 证书

1. 登录 [developer.apple.com](https://developer.apple.com) → **Certificates, Identifiers & Profiles**
2. 点 **+** → **Developer ID Application**
3. 在 Mac 上通过 **Keychain Access → Certificate Assistant → Request a Certificate from a Certificate Authority**
   - 输入邮箱和姓名
   - 选择 **Saved to disk** → **Let me specify key pair**
   - 密钥大小至少 **2048 位**，算法 **RSA**
4. 上传生成的 `.certSigningRequest`，下载 `.cer` 文件
5. 双击 `.cer` 导入 Keychain
6. 在 Keychain 中找到该证书（名称格式：`Developer ID Application: Your Name (TEAMID)`）
7. 右键 → **Export "..."** → 格式 **Personal Information Exchange (.p12)**
   - 设置导出密码（后续需要用到）
8. Base64 编码（放到剪贴板）：
   ```bash
   base64 -i certificate.p12 | pbcopy
   ```

### 2. 创建 App Store Connect API 密钥

1. 登录 [appstoreconnect.apple.com](https://appstoreconnect.apple.com)
2. 进入 **Users and Access** → **Keys** 标签 → **API Keys**
3. 点 **+** 新建密钥
   - 名称：`EasyBot Notarization`
   - 权限：**Developer**（不需要 Admin）
4. 下载 `.p8` 文件（**仅有一次下载机会，务必保存好**）
5. 记录以下信息（页面展示，下载后不会再次显示）：
   - **Key ID** （如 `ABC123DEFG`）
   - **Issuer ID** （UUID 格式，如 `12345678-1234-1234-1234-123456789012`）
6. Base64 编码 `.p8` 文件：
   ```bash
   base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy
   ```

### 3. 在 GitHub 仓库添加 Secrets

转到仓库 **Settings → Secrets and variables → Actions → New repository secret**，添加以下 5 个 Secret：

| Secret | 值 | 来源 |
|--------|-----|------|
| `APPLE_DEVELOPER_ID_CERT_BASE64` | p12 文件的 base64 编码 | 步骤 1.8 |
| `APPLE_DEVELOPER_ID_CERT_PASSWORD` | 导出 p12 时设置的密码 | 步骤 1.7 |
| `APPLE_NOTARY_API_KEY_BASE64` | p8 密钥文件的 base64 编码 | 步骤 2.6 |
| `APPLE_NOTARY_API_KEY_ID` | API Key ID（如 `ABC123DEFG`） | 步骤 2.5 |
| `APPLE_NOTARY_API_ISSUER` | Issuer ID（UUID） | 步骤 2.5 |

### 4. 验证

1. 触发 Release 工作流：**Actions → Release → Run workflow**
2. 在 `build-binaries` job 中，`*-apple-darwin` 目标会依次执行：
   - **签名**：`codesign` 使用 Developer ID 证书签名，启用 Hardened Runtime
   - **公证**：`notarytool` 将二进制提交给 Apple 扫描
   - **贴票**：`stapler` 将公证票据嵌入二进制
3. 签名/公证步骤仅在 **所有 5 个 Secret 都存在** 时执行，缺少任意一个则跳过

## 不设置凭据时

二进制正常构建但**不签名、不公证**。用户可在 macOS 上通过**右键 → 打开**跳过 Gatekeeper 运行。

## 常见问题

### 证书过期了怎么办

Developer ID Application 证书有效期为 **5 年**（2025 年后改为 5 年）。过期后重新执行整个流程即可。

### API 密钥泄露了怎么办

在 App Store Connect → **Users and Access → Keys** 中废止对应的 API 密钥，重新生成一个。

### 公证失败了怎么办

检查以下几点：
1. 二进制是否启用了 `com.apple.security.get-task-allow` 或其他测试 entitlement？（不应有）
2. 代码签名是否包含 `--timestamp` 和 `--options runtime`？（CI 脚本已包含）
3. 网络是否能访问 Apple 的公证服务？（CI runner 通常可以）

## 参考链接

- [Apple Developer Documentation: Distributing Mac Apps Outside the Mac App Store](https://developer.apple.com/documentation/macos-release-notes/macos-_14-release-notes/appkit-release-notes-for-macos-14)
- [notarytool 命令行](https://developer.apple.com/documentation/notarytool)
- [Code Signing Guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/)
