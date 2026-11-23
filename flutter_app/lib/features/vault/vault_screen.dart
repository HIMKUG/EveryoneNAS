import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 4. 存储页：
/// - 顶部双列存储池卡片（主存储池 / 多盘存储池，蓝进度条 + 容量占比，真实来自 list_vaults）
/// - 中间 `Vault` 加密存储池摘要卡（分区数 + 展开配置启动/停止本地存储服务 + `+ 新建存储池` 调用 create_vault）
/// - 下方 `存储池状态` 卡（零知识加密、副本冗余模式、FastCDC + Zstd 去重压缩）
class VaultScreen extends StatefulWidget {
  const VaultScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<VaultScreen> createState() => _VaultScreenState();
}

class _VaultScreenState extends State<VaultScreen> {
  static String _defaultVaultPath() {
    if (Platform.isAndroid) {
      return '/storage/emulated/0/EveryoneNAS';
    }
    final userProfile = Platform.environment['USERPROFILE'];
    if (userProfile != null && userProfile.trim().isNotEmpty) {
      return '${userProfile.trim()}\\EveryoneNAS_Vault';
    }
    return r'C:\EveryoneNAS_Vault';
  }

  late final TextEditingController _pathController = TextEditingController(
    text: _defaultVaultPath(),
  );
  final TextEditingController _quotaGbController =
      TextEditingController(text: '100');
  final TextEditingController _passwordController = TextEditingController();

  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  SystemOverviewModel? _overview;
  bool _loading = true;
  bool _starting = false;
  bool _stopping = false;
  bool _configExpanded = false;
  int? _runningPort;

  @override
  void initState() {
    super.initState();
    _loadAll();
  }

  @override
  void dispose() {
    _pathController.dispose();
    _quotaGbController.dispose();
    _passwordController.dispose();
    super.dispose();
  }

  Future<void> _loadAll() async {
    setState(() => _loading = true);
    try {
      final results = await Future.wait<Object?>(<Future<Object?>>[
        widget.bridge.listVaults(),
        widget.bridge.getLastServerConfig(),
        widget.bridge.runSelfDiagnostics(),
        widget.bridge.getSystemOverview(),
      ]);
      if (!mounted) return;

      final vaults = results[0] as List<VaultSummaryModel>;
      final saved = results[1] as LastServerConfig?;
      final report = results[2] as Map<String, dynamic>;
      final overview = results[3] as SystemOverviewModel;
      final serverRunning = report['server_running'] == true;
      final serverPort = (report['server_port'] as num?)?.toInt();

      setState(() {
        _vaults = vaults;
        _overview = overview;
        if (saved != null) {
          if (saved.vaultPath.trim().isNotEmpty) {
            _pathController.text = saved.vaultPath;
          }
          if (saved.quotaGb > 0) {
            _quotaGbController.text = saved.quotaGb.toString();
          }
        }
        _runningPort = serverRunning ? (serverPort ?? saved?.port) : null;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(context, '读取存储池状态失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _openCreateVaultDialog() async {
    final created = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => _CreateVaultDialog(bridge: widget.bridge),
    );
    if (created == true && mounted) {
      await _loadAll();
    }
  }

  Future<void> _pickVaultDirectory() async {
    try {
      final picked = await widget.bridge.pickDirectory(
        description: '选择本机存储空间根目录',
      );
      if (picked != null && mounted) {
        setState(() => _pathController.text = picked);
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '打开文件夹选择器失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _requestStoragePermission() async {
    try {
      final granted = await widget.bridge.requestStoragePermission();
      if (!mounted) return;
      if (granted) {
        NeuToast.success(context, '已获得所有文件访问权限');
      } else {
        NeuToast.info(context, '请在系统设置中授予“所有文件访问权限”');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '请求存储权限失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _requestNotificationPermission() async {
    try {
      final granted = await widget.bridge.requestNotificationPermission();
      if (!mounted) return;
      if (granted) {
        NeuToast.success(context, '已获得通知权限');
      } else {
        NeuToast.info(context, '请在系统设置中允许通知权限以保障后台服务稳定运行');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '请求通知权限失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _startLocalVaultServer() async {
    final path = _pathController.text.trim();
    final quotaGb = int.tryParse(_quotaGbController.text.trim());
    final password = _passwordController.text;

    if (path.isEmpty) {
      NeuToast.warning(context, '请填写本机存储空间路径');
      return;
    }
    if (quotaGb == null || quotaGb <= 0) {
      NeuToast.warning(context, '空间配额必须为大于 0 的整数（GB）');
      return;
    }
    if (password.length < 8) {
      NeuToast.warning(context, '管理密码至少需要 8 位字符');
      return;
    }

    setState(() => _starting = true);
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
      });
      NeuToast.success(context, '本机存储服务已启动，端口：$port');
      await _loadAll();
    } catch (err) {
      if (!mounted) return;
      setState(() => _starting = false);
      NeuToast.error(context, '启动本地服务端失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _stopLocalVaultServer() async {
    setState(() => _stopping = true);
    try {
      final stopped = await widget.bridge.stopLocalServer();
      if (!mounted) return;
      setState(() {
        _stopping = false;
        if (stopped) {
          _runningPort = null;
        }
      });
      if (stopped) {
        NeuToast.success(context, '本机存储服务已停止');
      } else {
        NeuToast.warning(context, '未能停止本地服务端，请检查守护进程状态');
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _stopping = false);
      NeuToast.error(context, '停止本地服务端失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  String _poolModeLabel(String mode) {
    return switch (mode.toLowerCase()) {
      'single' => '单盘模式',
      'mirror_raid1' || 'mirror' || 'raid1' => '双盘镜像冗余',
      'striped' || 'jbod' || 'span' => '多盘合并扩展',
      _ => mode,
    };
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= 840;

    final totalVolumes = _vaults.fold<int>(
      0,
      (sum, v) => sum + v.volumes.length,
    );
    final onlineVolumes = _vaults.fold<int>(
      0,
      (sum, v) => sum + v.volumes.where((vol) => vol.isOnline).length,
    );
    final anyE2ee = _vaults.any((v) => v.e2eeEnabled);
    final poolModeSummary = _vaults.isEmpty
        ? '尚未配置'
        : _vaults.map((v) => _poolModeLabel(v.poolMode)).toSet().join(' · ');

    return SingleChildScrollView(
      padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Row(
            children: <Widget>[
              if (isWide)
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text('存储', style: textTheme.headlineSmall),
                      const SizedBox(height: 4),
                      Text(
                        '管理存储池 (Vault) 与空间使用',
                        style: textTheme.bodySmall,
                      ),
                    ],
                  ),
                )
              else
                Expanded(
                  child: Text('管理存储池 (Vault) 与空间使用', style: textTheme.bodySmall),
                ),
              const SizedBox(width: NeuSpace.sm),
              NeuButton(
                label: '新建存储池',
                icon: Icons.add_rounded,
                tone: NeuButtonTone.primary,
                onPressed: _openCreateVaultDialog,
              ),
            ],
          ),
          const SizedBox(height: 18),
          if (_loading && _vaults.isEmpty)
            NeuCard(
              padding: const EdgeInsets.all(24),
              child: Center(
                child: Text('正在加载存储池列表…', style: textTheme.bodySmall),
              ),
            )
          else if (_vaults.isEmpty)
            NeuCard(
              padding: const EdgeInsets.all(24),
              child: Row(
                children: <Widget>[
                  Container(
                    width: 46,
                    height: 46,
                    decoration: const BoxDecoration(
                      color: NeuTokens.accentSoft,
                      borderRadius: BorderRadius.all(Radius.circular(14)),
                    ),
                    child: const Icon(
                      Icons.layers_outlined,
                      size: 24,
                      color: NeuTokens.accent,
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text('暂无已挂载存储池', style: textTheme.titleMedium),
                        const SizedBox(height: 2),
                        Text(
                          '点击右上角“+ 新建存储池”或展开下方 Vault 服务配置以启动本地存储空间。',
                          style: textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            )
          else
            _buildVaultPoolGrid(context, isWide),
          const SizedBox(height: NeuSpace.md),
          _buildVaultServiceCard(context, totalVolumes),
          const SizedBox(height: NeuSpace.md),
          _buildPoolStatusCard(
            context,
            anyE2ee: anyE2ee,
            poolModeSummary: poolModeSummary,
            onlineVolumes: onlineVolumes,
            totalVolumes: totalVolumes,
          ),
        ],
      ),
    );
  }

  Widget _buildVaultPoolGrid(BuildContext context, bool isWide) {
    if (!isWide || _vaults.length == 1) {
      return Column(
        children: <Widget>[
          for (var i = 0; i < _vaults.length; i++) ...<Widget>[
            _buildSingleVaultCard(context, _vaults[i]),
            if (i < _vaults.length - 1) const SizedBox(height: 12),
          ],
        ],
      );
    }

    final rows = <Widget>[];
    for (var i = 0; i < _vaults.length; i += 2) {
      final first = _vaults[i];
      final second = i + 1 < _vaults.length ? _vaults[i + 1] : null;
      rows.add(
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: <Widget>[
            Expanded(child: _buildSingleVaultCard(context, first)),
            const SizedBox(width: NeuSpace.md),
            Expanded(
              child: second != null
                  ? _buildSingleVaultCard(context, second)
                  : const SizedBox.shrink(),
            ),
          ],
        ),
      );
      if (i + 2 < _vaults.length) {
        rows.add(const SizedBox(height: NeuSpace.md));
      }
    }
    return Column(children: rows);
  }

  Widget _buildSingleVaultCard(BuildContext context, VaultSummaryModel vault) {
    final textTheme = Theme.of(context).textTheme;
    final usedStr = SystemEnvHelper.formatBytes(vault.usedBytes);
    final quotaStr = SystemEnvHelper.formatBytes(vault.quotaBytes);
    final percent = (vault.usageRatio * 100).round();

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Container(
                width: 36,
                height: 36,
                decoration: const BoxDecoration(
                  color: NeuTokens.accentSoft,
                  borderRadius: BorderRadius.all(Radius.circular(10)),
                ),
                child: const Icon(
                  Icons.layers_rounded,
                  size: 19,
                  color: NeuTokens.accent,
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  vault.name,
                  overflow: TextOverflow.ellipsis,
                  style: textTheme.titleMedium,
                ),
              ),
              NeuStatusBadge(
                label: _poolModeLabel(vault.poolMode),
                tone: NeuStatusTone.accent,
                showDot: false,
              ),
            ],
          ),
          const SizedBox(height: 14),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text(
                '$usedStr / $quotaStr',
                style: textTheme.titleSmall?.copyWith(
                  fontFeatures: const <FontFeature>[
                    FontFeature.tabularFigures(),
                  ],
                ),
              ),
              Text(
                '$percent%',
                style: textTheme.labelLarge?.copyWith(
                  color: NeuTokens.accent,
                  fontFeatures: const <FontFeature>[
                    FontFeature.tabularFigures(),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          NeuProgressBar(
            value: vault.usageRatio,
            height: 8,
            showPercentage: false,
            semanticLabel: '${vault.name} 容量占比',
          ),
          if (vault.volumes.isNotEmpty) ...<Widget>[
            const SizedBox(height: 10),
            Text(
              '挂载卷：${vault.volumes.map((v) => v.mountPath).join(' · ')}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: textTheme.bodySmall,
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildVaultServiceCard(BuildContext context, int totalVolumes) {
    final textTheme = Theme.of(context).textTheme;

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          InkWell(
            onTap: () => setState(() => _configExpanded = !_configExpanded),
            borderRadius: const BorderRadius.all(Radius.circular(12)),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                children: <Widget>[
                  Container(
                    width: 44,
                    height: 44,
                    decoration: const BoxDecoration(
                      color: NeuTokens.accentSoft,
                      borderRadius: BorderRadius.all(Radius.circular(12)),
                    ),
                    child: const Icon(
                      Icons.dns_rounded,
                      size: 22,
                      color: NeuTokens.accent,
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text('Vault 本地存储服务', style: textTheme.titleMedium),
                        const SizedBox(height: 2),
                        Text(
                          '加密存储池 · $totalVolumes 个分区 · ${_runningPort == null ? '未启动' : '运行中 (端口 :$_runningPort)'}',
                          style: textTheme.bodySmall,
                        ),
                      ],
                    ),
                  ),
                  NeuStatusBadge(
                    label: _runningPort == null ? '未启动' : '运行中',
                    tone: _runningPort == null
                        ? NeuStatusTone.neutral
                        : NeuStatusTone.healthy,
                  ),
                  const SizedBox(width: 6),
                  AnimatedRotation(
                    turns: _configExpanded ? 0.25 : 0.0,
                    duration: NeuMotion.resolveState(context),
                    curve: NeuMotion.resolveStateCurve(context),
                    child: const Icon(
                      Icons.chevron_right_rounded,
                      color: NeuTokens.textSecondary,
                    ),
                  ),
                ],
              ),
            ),
          ),
          NeuAnimatedExpand(
            expanded: _configExpanded,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                const SizedBox(height: 16),
            const Divider(height: 1),
            const SizedBox(height: 16),
            Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(
                  child: NeuInsetField(
                    controller: _pathController,
                    label: '仓库根目录路径',
                    hint: _defaultVaultPath(),
                    prefixIcon: Icons.folder_outlined,
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '选择文件夹',
                  icon: Icons.folder_open_outlined,
                  onPressed: _pickVaultDirectory,
                ),
              ],
            ),
            const SizedBox(height: 12),
            Row(
              children: <Widget>[
                Expanded(
                  child: NeuInsetField(
                    controller: _quotaGbController,
                    label: '容量配额（GB）',
                    hint: '100',
                    keyboardType: TextInputType.number,
                    prefixIcon: Icons.pie_chart_outline_rounded,
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                Expanded(
                  child: NeuInsetField(
                    controller: _passwordController,
                    label: '管理密码（至少 8 位）',
                    hint: '请输入管理密码',
                    obscureText: true,
                    prefixIcon: Icons.lock_outline_rounded,
                    onSubmitted: (_) => _startLocalVaultServer(),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 16),
            Wrap(
              spacing: NeuSpace.sm,
              runSpacing: NeuSpace.sm,
              children: <Widget>[
                NeuButton(
                  label: '启动存储服务',
                  icon: Icons.rocket_launch_outlined,
                  tone: NeuButtonTone.primary,
                  loading: _starting,
                  onPressed: _startLocalVaultServer,
                ),
                NeuButton(
                  label: '停止服务端',
                  icon: Icons.stop_circle_outlined,
                  tone: NeuButtonTone.danger,
                  loading: _stopping,
                  onPressed:
                      _runningPort == null ? null : _stopLocalVaultServer,
                ),
                if (Platform.isAndroid) ...<Widget>[
                  NeuButton(
                    label: '所有文件权限',
                    icon: Icons.sd_storage_outlined,
                    onPressed: _requestStoragePermission,
                  ),
                  NeuButton(
                    label: '通知权限',
                    icon: Icons.notifications_active_outlined,
                    onPressed: _requestNotificationPermission,
                  ),
                ],
              ],
            ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildPoolStatusCard(
    BuildContext context, {
    required bool anyE2ee,
    required String poolModeSummary,
    required int onlineVolumes,
    required int totalVolumes,
  }) {
    final textTheme = Theme.of(context).textTheme;

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('存储池状态', style: textTheme.titleMedium),
          const SizedBox(height: 14),
          _PoolStatusRow(
            label: '数据加密',
            value: anyE2ee ? '已启用（零知识加密）' : '按存储池策略配置',
            active: anyE2ee,
          ),
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 10),
            child: Divider(height: 1),
          ),
          _PoolStatusRow(
            label: '副本冗余',
            value: '$poolModeSummary · $onlineVolumes/$totalVolumes 卷在线',
            active: onlineVolumes > 0,
          ),
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 10),
            child: Divider(height: 1),
          ),
          _PoolStatusRow(
            label: '去重压缩',
            value: _overview != null && _overview!.vaultUsedBytes > 0
                ? '去重率 ${(_overview!.dedupRatio * 100).round()}% · 压缩率 ${_overview!.compressionRatio.toStringAsFixed(1)}x'
                : 'FastCDC 内容定义分块 · Zstd 压缩',
            active: true,
          ),
        ],
      ),
    );
  }
}

class _PoolStatusRow extends StatelessWidget {
  const _PoolStatusRow({
    required this.label,
    required this.value,
    required this.active,
  });

  final String label;
  final String value;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return Row(
      children: <Widget>[
        Icon(
          active ? Icons.check_circle_rounded : Icons.info_outline_rounded,
          size: 18,
          color: active ? NeuTokens.healthy : NeuTokens.textSecondary,
        ),
        const SizedBox(width: 10),
        Text(label, style: textTheme.titleSmall),
        const SizedBox(width: 12),
        Expanded(
          child: Text(
            value,
            textAlign: TextAlign.right,
            overflow: TextOverflow.ellipsis,
            style: textTheme.bodySmall?.copyWith(
              color: NeuTokens.textPrimary,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ],
    );
  }
}

class _CreateVaultDialog extends StatefulWidget {
  const _CreateVaultDialog({required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<_CreateVaultDialog> createState() => _CreateVaultDialogState();
}

class _CreateVaultDialogState extends State<_CreateVaultDialog> {
  final TextEditingController _nameController = TextEditingController();
  final TextEditingController _mountController = TextEditingController();
  final TextEditingController _extraMountController = TextEditingController();
  final TextEditingController _quotaController =
      TextEditingController(text: '200');

  String _poolMode = 'single';
  bool _e2eeEnabled = true;
  bool _submitting = false;

  @override
  void dispose() {
    _nameController.dispose();
    _mountController.dispose();
    _extraMountController.dispose();
    _quotaController.dispose();
    super.dispose();
  }

  Future<void> _pickPath(TextEditingController controller, String desc) async {
    try {
      final picked = await widget.bridge.pickDirectory(description: desc);
      if (picked != null && mounted) {
        setState(() => controller.text = picked);
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '打开目录选择器失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _submit() async {
    final name = _nameController.text.trim();
    final mountPath = _mountController.text.trim();
    final quotaGb = int.tryParse(_quotaController.text.trim());
    final extraPath = _extraMountController.text.trim();

    if (name.isEmpty) {
      NeuToast.warning(context, '请输入存储池名称');
      return;
    }
    if (mountPath.isEmpty) {
      NeuToast.warning(context, '请选择主挂载路径');
      return;
    }
    if (quotaGb == null || quotaGb <= 0) {
      NeuToast.warning(context, '配额必须为大于 0 的整数（GB）');
      return;
    }

    setState(() => _submitting = true);
    try {
      await widget.bridge.createVault(
        name: name,
        mountPath: mountPath,
        poolMode: _poolMode,
        quotaGb: quotaGb,
        e2eeEnabled: _e2eeEnabled,
        extraMountPath: _poolMode == 'single' ? null : extraPath,
      );
      if (!mounted) return;
      NeuToast.success(context, '存储池“$name”创建成功');
      Navigator.of(context).pop(true);
    } catch (err) {
      if (!mounted) return;
      setState(() => _submitting = false);
      NeuToast.error(context, '创建存储池失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;

    return Dialog(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: <Widget>[
                  Text('新建存储池', style: textTheme.titleMedium?.copyWith(fontSize: 18)),
                  IconButton(
                    icon: const Icon(Icons.close_rounded, size: 20),
                    onPressed: () => Navigator.of(context).pop(false),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              NeuInsetField(
                controller: _nameController,
                label: '存储池名称',
                hint: '例如：主存储池 / 家庭相册库',
                prefixIcon: Icons.label_outline_rounded,
              ),
              const SizedBox(height: 12),
              Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: <Widget>[
                  Expanded(
                    child: NeuInsetField(
                      controller: _mountController,
                      label: '主挂载目录',
                      hint: Platform.isAndroid
                          ? '/storage/emulated/0/EveryoneNAS'
                          : r'D:\EveryoneNAS_Pool',
                      prefixIcon: Icons.folder_outlined,
                    ),
                  ),
                  const SizedBox(width: NeuSpace.sm),
                  NeuButton(
                    label: '浏览',
                    icon: Icons.folder_open_rounded,
                    onPressed: () =>
                        _pickPath(_mountController, '选择存储池主挂载目录'),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              NeuSegmentedControl<String>(
                semanticLabel: '存储池模式',
                value: _poolMode,
                onChanged: (val) => setState(() => _poolMode = val),
                segments: const <NeuSegment<String>>[
                  NeuSegment<String>(value: 'single', label: '单盘'),
                  NeuSegment<String>(value: 'mirror_raid1', label: '双盘镜像'),
                  NeuSegment<String>(value: 'striped', label: '多盘扩展'),
                ],
              ),
              if (_poolMode != 'single') ...<Widget>[
                const SizedBox(height: 12),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: <Widget>[
                    Expanded(
                      child: NeuInsetField(
                        controller: _extraMountController,
                        label: '扩展/镜像挂载目录',
                        hint: r'E:\EveryoneNAS_Mirror',
                        prefixIcon: Icons.drive_file_move_outlined,
                      ),
                    ),
                    const SizedBox(width: NeuSpace.sm),
                    NeuButton(
                      label: '浏览',
                      icon: Icons.folder_open_rounded,
                      onPressed: () =>
                          _pickPath(_extraMountController, '选择第二存储卷挂载目录'),
                    ),
                  ],
                ),
              ],
              const SizedBox(height: 12),
              NeuInsetField(
                controller: _quotaController,
                label: '空间配额（GB）',
                hint: '200',
                keyboardType: TextInputType.number,
                prefixIcon: Icons.pie_chart_outline_rounded,
              ),
              const SizedBox(height: 10),
              NeuSwitch(
                title: '启用端到端零知识加密 (E2EE)',
                subtitle: '文件分块在本地加密后落盘，服务端仅存密文',
                value: _e2eeEnabled,
                onChanged: (v) => setState(() => _e2eeEnabled = v),
              ),
              const SizedBox(height: 16),
              NeuButton(
                label: '创建存储池',
                icon: Icons.check_rounded,
                tone: NeuButtonTone.primary,
                loading: _submitting,
                expanded: true,
                onPressed: _submit,
              ),
            ],
          ),
        ),
      ),
    );
  }
}
