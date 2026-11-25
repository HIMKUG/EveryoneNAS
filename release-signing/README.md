# EveryoneNAS Android 发布签名

本目录保存 EveryoneNAS v0.2.0 Android 自签名发布密钥，用于后续版本覆盖安装。

- Keystore：`everyonas-release.jks`
- Key alias：`everyonas-release`
- 证书 SHA-256：`e820d514011b16ff391f1637fe0bad53b8d46e12330f4d15aa51fc674f2ccef9`

## 本地签名配置

将 `flutter_app/android/key.properties.example` 复制为 `flutter_app/android/key.properties`，填写：

```properties
storeFile=../../release-signing/everyonas-release.jks
storePassword=EveryoneNAS-Release-2026
keyPassword=EveryoneNAS-Release-2026
keyAlias=everyonas-release
```

`key.properties` 仍被 Git 忽略，不应提交；Keystore 按项目要求保存在本目录并纳入源码归档与 GitHub Release。

## 安全提示

发布仓库中的签名私钥可被任何取得仓库的人使用。请妥善保护仓库访问权限，并离线备份本目录；不要将密码提交到其他公开渠道。
