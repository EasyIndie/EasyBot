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
   - **验证**：`notarytool` 返回 Accepted；原始 Mach-O 可执行文件不支持 stapling，Gatekeeper 在线查询公证票据
3. 签名步骤在证书 Secret 存在时执行；配置公证密钥后再提交 Apple 公证。若需要可离线验证的 stapled 票据，应改为发布 `.pkg` 或 `.app`，而不是裸可执行文件。

## 不设置凭据时

**默认行为是「告警 + 继续发布未签名的 macOS 二进制」**，不是中止发布：

```
##[warning]Apple signing/notarization credentials are incomplete;
           publishing an UNSIGNED macOS binary (users will hit a Gatekeeper prompt).
```

未签名的 macOS 产物用户首次运行会被 Gatekeeper 拦下，需要右键 → 打开，或：

```bash
xattr -d com.apple.quarantine ./easybot
```

若要让发布在缺凭据时**硬失败**（推荐用于正式商业发布），在调用处传 `on-missing: fail`：

```yaml
        uses: ./.github/actions/macos-sign-notarize
        with:
          binary: target/${{ matrix.target }}/release/easybot${{ matrix.suffix }}
          on-missing: fail          # ← 缺凭据直接报错，不发布未签名产物
          cert-base64: ${{ secrets.APPLE_DEVELOPER_ID_CERT_BASE64 }}
          # ...
```

> ⚠️ 历史文档曾声称「商业 Release 会失败并停止发布」——那是**错误的**：工作流里没有任何强制门禁，
> 只有上述告警。现已改为可显式选择 `warn`（默认，保持向后兼容）/ `fail`。

## 在其它项目复用

签名 + 公证逻辑已抽成 composite action **`.github/actions/macos-sign-notarize`**，凭据全部走
inputs（不绑定本仓库的 secret 名），因此可直接被其它仓库/其它 job 调用：

```yaml
      - name: Sign and notarize macOS binary
        if: contains(matrix.target, 'apple-darwin')
        uses: EasyIndie/EasyBot/.github/actions/macos-sign-notarize@v0.0.43
        with:
          binary: target/${{ matrix.target }}/release/myapp
          cert-base64: ${{ secrets.APPLE_DEVELOPER_ID_CERT_BASE64 }}
          cert-password: ${{ secrets.APPLE_DEVELOPER_ID_CERT_PASSWORD }}
          notary-api-key: ${{ secrets.APPLE_NOTARY_API_KEY_BASE64 }}
          notary-key-id: ${{ secrets.APPLE_NOTARY_API_KEY_ID }}
          notary-issuer: ${{ secrets.APPLE_NOTARY_API_ISSUER }}
```

**关于「每个项目都要重做一遍吗」**：不用。Developer ID 证书与 App Store Connect API Key 都是
**按 Apple Developer 团队**签发的（不是按项目）——生成一次即可被所有 macOS 应用复用，证书 5 年有效。
真正重复的只是「把同一组 5 个值填进每个仓库的 secrets」，可通过下列方式消除：

| 方式 | 适用 |
|---|---|
| **Organization / Environment secrets** | 同一 org 内的所有仓库共享，零复制；轮换只改一处（推荐） |
| `gh secret set <NAME> -R <repo> < file` 脚本 | 跨 org / 个人账号仓库 |
| fastlane `match` | 把证书存进加密的私有仓库或对象存储，CI 里自动安装（跨 CI 平台） |

> 建议用**团队级密码管理器**保存 `certificate.p12` / `.p8` 与密码本体——它们是团队资产，不是项目资产。

## Windows（Authenticode 签名）

对称地，Windows 产物用 `.github/actions/windows-sign`（同为 composite action，凭据走 inputs）：

| Secret | 值 |
|---|---|
| `WINDOWS_CODE_SIGNING_CERT_BASE64` | 代码签名证书（`.pfx`，base64 编码） |
| `WINDOWS_CODE_SIGNING_CERT_PASSWORD` | 导出 pfx 时的密码 |

```yaml
      - name: Sign Windows binary
        if: contains(matrix.target, 'windows-msvc')
        uses: EasyIndie/EasyBot/.github/actions/windows-sign@v0.0.43
        with:
          binary: target/${{ matrix.target }}/release/myapp.exe
          cert-base64: ${{ secrets.WINDOWS_CODE_SIGNING_CERT_BASE64 }}
          cert-password: ${{ secrets.WINDOWS_CODE_SIGNING_CERT_PASSWORD }}
```

行为与 macOS 侧一致：缺凭据默认**告警 + 发布未签名产物**，传 `on-missing: fail` 改为硬失败；
签名后独立复核（`Get-AuthenticodeSignature`），并检查是否带时间戳证书。

> 证书与 macOS 的 Developer ID 类似是**团队级资产**（OV/EV 代码签名证书，一般 1–3 年有效），
> 所有 Windows 应用共用一份。未签名的 Windows 产物用户会看到 SmartScreen「未知发布者」提示。

## 没有 Mac 时：用 `rcodesign` 替代

上面第 1 步依赖 macOS 的 Keychain Access。若手上没有 Mac，可用纯 Rust 的
[`rcodesign`](https://crates.io/crates/apple-codesign)（`cargo install apple-codesign`）：
在 Linux/Windows 上生成 CSR、用 PEM 私钥 + 证书直接签名、并提交公证（`rcodesign notary-submit`），
无需手工拼 p12。

**用手工 `openssl` 合成 p12 时的坑**：OpenSSL 3.x 默认用 AES-256/PBKDF2，macOS 的
`security import` 解不开（报 `MAC verification failed`）。必须显式指定旧算法，并带上 Apple 的
Developer ID G2 中间证书：

```bash
curl -fsSLO https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
openssl x509 -inform DER -in DeveloperIDG2CA.cer -out DeveloperIDG2CA.pem
cat cert.pem DeveloperIDG2CA.pem > chain.pem
openssl pkcs12 -export -inkey developerid.key -in chain.pem -out certificate.p12 \
  -name "Developer ID Application" -passout "pass:$P12_PASSWORD" \
  -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1
```

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
