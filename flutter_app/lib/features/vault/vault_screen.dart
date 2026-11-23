// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 4 / 5 / 6 节 (start_local_server, requestStoragePermission)
// - lanvault_complete_blueprint.md 第 3.1 / 3.5 节 (本机存储空间启动、配额、Web 密码与快照时间轴入口)

import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';

/// 本机存储空间配置、服务端启动与 GFS 快照浏览入口页面。
class VaultScreen extends StatefulWidget {
  const VaultScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<VaultScreen> createState() => _VaultScreenState();
}

class _VaultScreenState extends State<VaultScreen> {
  final TextEditingController _pathController = TextEditingController(
    text: Platform.isAndroid
        ? '/storage/emulated/0/EveryoneNAS'
        : r'D:\EveryoneNAS',
  );
  final TextEditingController _quotaGbController = TextEditingController(text: '100');
  final TextEditingController _passwordController = TextEditingController();

  bool _starting = false;
  int? _runningPort;
  String? _feedback;
  String? _error;

  @override
  void dispose() {
    _pathController.dispose();
    _quotaGbController.dispose();
    _passwordController.dispose();
    super.dispose();
  }

  Future<void> _requestStoragePermission() async {
    setState(() {
      _error = null;
      _feedback = null;
    });
    try {
      final granted = await widget.bridge.requestStoragePermission();
      if (!mounted) return;
      setState(() {
        _feedback = granted
            ? '已获得 MANAGE_EXTERNAL_STORAGE 所有文件访问权限。'
            : '请在系统设置中授予“所有文件访问权限”以启用 POSIX 直连存储。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _error = '请求存储权限失败：$err');
    }
  }

  Future<void> _startLocalVaultServer() async {
    final path = _pathController.text.trim();
    final quotaGb = int.tryParse(_quotaGbController.text.trim());
    final password = _passwordController.text;

    if (path.isEmpty) {
      setState(() => _error = '请填写本机存储空间路径。');
      return;
    }
    if (quotaGb == null || quotaGb <= 0) {
      setState(() => _error = '空间配额必须为大于 0 的整数（GB）。');
      return;
    }
    if (password.length < 6) {
      setState(() => _error = 'Web 管理台密码至少需要 6 位字符。');
      return;
    }

    setState(() {
      _starting = true;
      _error = null;
      _feedback = null;
    });

    try {
      final port = await widget.bridge.startLocalServer(
        vaultPath: path,
        quotaGb: quotaGb,
        webPassword: password,
      );
      if (!mounted) return;
      _passwordController.clear();
      setState(() {
        _starting = false;
        _runningPort = port;
        _feedback = '本机存储空间与 Web 管理台已启动，监听端口：$port。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _starting = false;
        _error = '启动本地服务端失败：$err';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;

    return SingleChildScrollView(
      padding: const EdgeInsets.all(NeuTokens.spaceLg),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text('本机存储空间', style: textTheme.headlineSmall),
          const SizedBox(height: NeuTokens.spaceXs),
          Text(
            '配置内容寻址仓库路径、容量配额与 Argon2id Web 管理密码，并进入 GFS 快照浏览',
            style: textTheme.bodySmall,
          ),
          const SizedBox(height: NeuTokens.spaceLg),
          if (_error != null) ...<Widget>[
            NeuCard(
              inset: true,
              child: Text(
                _error!,
                style: textTheme.bodyMedium?.copyWith(color: NeuTokens.danger),
              ),
            ),
            const SizedBox(height: NeuTokens.spaceMd),
          ],
          if (_feedback != null) ...<Widget>[
            NeuCard(
              inset: true,
              child: Text(
                _feedback!,
                style: textTheme.bodyMedium?.copyWith(color: NeuTokens.healthy),
              ),
            ),
            const SizedBox(height: NeuTokens.spaceMd),
          ],
          NeuCard(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text('启动本机存储服务端', style: textTheme.titleMedium),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuInsetField(
                  controller: _pathController,
                  label: '仓库根目录路径',
                  hint: '/storage/emulated/0/EveryoneNAS',
                  prefixIcon: Icons.folder_outlined,
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuInsetField(
                  controller: _quotaGbController,
                  label: '容量配额（GB）',
                  hint: '100',
                  keyboardType: TextInputType.number,
                  prefixIcon: Icons.pie_chart_outline,
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuInsetField(
                  controller: _passwordController,
                  label: 'Web 管理台密码（连续输错 3 次锁定 60 秒）',
                  hint: '至少 6 位密码',
                  obscureText: true,
                  prefixIcon: Icons.lock_outline,
                  onSubmitted: (_) => _startLocalVaultServer(),
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                Wrap(
                  spacing: NeuTokens.spaceSm,
                  runSpacing: NeuTokens.spaceSm,
                  children: <Widget>[
                    NeuButton(
                      label: '启动存储空间',
                      icon: Icons.rocket_launch_outlined,
                      tone: NeuButtonTone.primary,
                      loading: _starting,
                      onPressed: _startLocalVaultServer,
                    ),
                    if (Platform.isAndroid)
                      NeuButton(
                        label: '申请所有文件权限',
                        icon: Icons.sd_storage_outlined,
                        onPressed: _requestStoragePermission,
                      ),
                  ],
                ),
              ],
            ),
          ),
          const SizedBox(height: NeuTokens.spaceMd),
          NeuCard(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text('GFS 快照时间轴入口', style: textTheme.titleMedium),
                const SizedBox(height: NeuTokens.spaceSm),
                Text(
                  _runningPort == null
                      ? '启动本机服务端后，可通过内嵌 Web 管理台浏览按「7 天每日 / 4 周每周 / 6 月每月」保留的历史快照并回滚单文件。'
                      : '当前 Web 管理台地址：http://127.0.0.1:$_runningPort（快照接口 /api/snapshots）。',
                  style: textTheme.bodySmall,
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuCard(
                  inset: true,
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: <Widget>[
                      Text('当前服务端口', style: textTheme.bodySmall),
                      Text(
                        _runningPort == null ? '未启动' : ':$_runningPort',
                        style: textTheme.titleSmall?.copyWith(
                          color: _runningPort == null
                              ? NeuTokens.textSecondary
                              : NeuTokens.healthy,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
