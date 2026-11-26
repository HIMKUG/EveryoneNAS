import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 存储页（重构版）：
/// - 唯一重心：管理存储池与容量配额。
/// - 彻底移除“仓库根目录路径 / 8位管理密码 / 启动停止服务”实现模型大表单，
///   以及散落于此的“所有文件权限 / 通知权限”按钮与纯静态技术说明卡。
/// - 本地存储服务随节点自动常驻；若未启动，点击状态胶囊即可一键拉起。
class VaultScreen extends StatefulWidget {
  const VaultScreen({
    super.key,
    required this.bridge,
    this.onOpenSettings,
  });

  final LanVaultBridge bridge;
  final VoidCallback? onOpenSettings;

  @override
  State<VaultScreen> createState() => _VaultScreenState();
}

class _VaultScreenState extends State<VaultScreen> {
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  bool _loading = true;
  bool _startingService = false;
  String? _loadError;
  int _refreshRequest = 0;

  @override
  void initState() {
    super.initState();
    widget.bridge.serverStateVersion.addListener(_onStateChanged);
    unawaited(_refreshVaults(silent: false));
  }

  @override
  void dispose() {
    widget.bridge.serverStateVersion.removeListener(_onStateChanged);
    super.dispose();
  }

  void _onStateChanged() {
    if (!mounted) return;
    unawaited(_refreshVaults(silent: true));
  }

  Future<void> _refreshVaults({required bool silent}) async {
    final request = ++_refreshRequest;
    if (!silent && mounted) {
      setState(() {
        _loading = true;
        _loadError = null;
      });
    }
    try {
      final list = await widget.bridge.listVaults();
      if (!mounted || request != _refreshRequest) return;
      setState(() {
        _vaults = list;
        _loadError = null;
        _loading = false;
      });
    } catch (err) {
      if (!mounted || request != _refreshRequest) return;
      final msg = LanVaultBridge.formatUserError(err);
      setState(() {
        _loadError = msg;
        _loading = false;
      });
      if (!silent) {
        NeuToast.error(context, msg);
      }
    }
  }

  Future<void> _ensureServiceRunning() async {
    if (widget.bridge.serverRunningNotifier.value || _startingService) return;
    setState(() => _startingService = true);
    try {
      final port = await widget.bridge.autostartLocalServer();
      if (!mounted) return;
      if (port != null) {
        NeuToast.success(context, '本地存储服务已就绪（:$port）');
        await _refreshVaults(silent: true);
      } else {
        NeuToast.info(context, '请先在设置中确认本地存储目录');
        widget.onOpenSettings?.call();
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    } finally {
      if (mounted) setState(() => _startingService = false);
    }
  }

  Future<void> _openCreateVaultDialog() async {
    if (!widget.bridge.serverRunningNotifier.value) {
      await _ensureServiceRunning();
      if (!mounted || !widget.bridge.serverRunningNotifier.value) return;
    }

    final nameController = TextEditingController();
    final pathController = TextEditingController();
    final extraPathController = TextEditingController();
    final quotaController = TextEditingController(text: '100');
    var poolMode = PoolMode.single;
    var submitting = false;

    await showNeuDialog<void>(
      context: context,
      builder: (dialogCtx) {
        return StatefulBuilder(
          builder: (context, setDialogState) {
            return NeuDialog(
              title: '新建存储池',
              subtitle: '选择存储目录并分配空间配额',
              maxWidth: 460,
              actions: <Widget>[
                NeuButton(
                  label: '取消',
                  onPressed: submitting
                      ? null
                      : () => Navigator.of(dialogCtx).pop(),
                ),
                NeuButton(
                  label: '创建存储池',
                  icon: Icons.add_rounded,
                  tone: NeuButtonTone.primary,
                  loading: submitting,
                  onPressed: submitting
                      ? null
                      : () async {
                          final name = nameController.text.trim();
                          final mountPath = pathController.text.trim();
                          final quotaGb =
                              int.tryParse(quotaController.text.trim()) ?? 0;
                          final extraPath = extraPathController.text.trim();

                          if (name.isEmpty) {
                            NeuToast.warning(context, '请输入存储池名称');
                            return;
                          }
                          if (mountPath.isEmpty) {
                            NeuToast.warning(context, '请选择存储目录');
                            return;
                          }
                          if (quotaGb <= 0) {
                            NeuToast.warning(context, '容量配额必须大于 0 GB');
                            return;
                          }
                          final needsExtraVolume =
                              poolMode != PoolMode.single;
                          if (needsExtraVolume && extraPath.isEmpty) {
                            NeuToast.warning(
                              context,
                              poolMode == PoolMode.mirrorRaid1
                                  ? '双盘镜像模式需要选择第二块磁盘目录'
                                  : '多盘串联模式需要选择要追加的磁盘目录',
                            );
                            return;
                          }
                          if (poolMode == PoolMode.mirrorRaid1) {
                            final k1 =
                                SystemEnvHelper.extractDriveKey(mountPath);
                            final k2 =
                                SystemEnvHelper.extractDriveKey(extraPath);
                            if (k1 != null && k1 == k2) {
                              NeuToast.warning(context, '双盘镜像需位于不同物理磁盘');
                              return;
                            }
                          }

                          setDialogState(() => submitting = true);
                          try {
                            await widget.bridge.createVault(
                              name: name,
                              mountPath: mountPath,
                              poolMode: poolMode,
                              quotaGb: quotaGb,
                              e2eeEnabled: true,
                              extraMountPath: needsExtraVolume ? extraPath : null,
                            );
                            if (dialogCtx.mounted) {
                              Navigator.of(dialogCtx).pop();
                            }
                            if (mounted) {
                              NeuToast.success(this.context, '已创建存储池「$name」');
                              await _refreshVaults(silent: true);
                            }
                          } catch (err) {
                            if (dialogCtx.mounted) setDialogState(() => submitting = false);
                            if (mounted) {
                              NeuToast.error(
                                this.context,
                                LanVaultBridge.formatUserError(err),
                              );
                            }
                          }
                        },
                ),
              ],
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  NeuInsetField(
                    controller: nameController,
                    label: '存储池名称',
                    hint: '例如：家庭照片库',
                    prefixIcon: Icons.layers_outlined,
                  ),
                  const SizedBox(height: 12),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: <Widget>[
                      Expanded(
                        child: NeuInsetField(
                          controller: pathController,
                          label: '存储目录',
                          hint: Platform.isAndroid
                              ? '/storage/emulated/0/EveryoneNAS'
                              : r'D:\EveryoneNAS',
                          prefixIcon: Icons.folder_outlined,
                        ),
                      ),
                      const SizedBox(width: 8),
                      NeuButton(
                        label: '选择',
                        onPressed: () async {
                          final picked = await widget.bridge.pickDirectory(
                            description: '选择存储池目录',
                          );
                          if (dialogCtx.mounted && picked != null && picked.isNotEmpty) {
                            pathController.text = picked;
                          }
                        },
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  NeuInsetField(
                    controller: quotaController,
                    label: '容量配额 (GB)',
                    hint: '100',
                    keyboardType: TextInputType.number,
                    prefixIcon: Icons.storage_rounded,
                  ),
                  if (!Platform.isAndroid) ...<Widget>[
                    const SizedBox(height: 12),
                    NeuSegmentedControl<PoolMode>(
                      semanticLabel: '存储池模式',
                      value: poolMode,
                      onChanged: (v) => setDialogState(() => poolMode = v),
                      segments: const <NeuSegment<PoolMode>>[
                        NeuSegment<PoolMode>(
                            value: PoolMode.single, label: '单盘模式'),
                        NeuSegment<PoolMode>(
                            value: PoolMode.striped, label: '多盘串联'),
                        NeuSegment<PoolMode>(
                            value: PoolMode.mirrorRaid1, label: '双盘镜像'),
                      ],
                    ),
                    if (poolMode != PoolMode.single) ...<Widget>[
                      const SizedBox(height: 12),
                      Row(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: <Widget>[
                          Expanded(
                            child: NeuInsetField(
                              controller: extraPathController,
                              label: poolMode == PoolMode.striped
                                  ? '追加磁盘目录'
                                  : '第二磁盘目录',
                              hint: poolMode == PoolMode.striped
                                  ? r'E:\EveryoneNAS_Ext'
                                  : r'E:\EveryoneNAS_Mirror',
                              prefixIcon: Icons.folder_copy_outlined,
                            ),
                          ),
                          const SizedBox(width: 8),
                          NeuButton(
                            label: '选择',
                            onPressed: () async {
                              final picked = await widget.bridge.pickDirectory(
                                description: poolMode == PoolMode.striped
                                    ? '选择要追加到存储池的磁盘目录'
                                    : '选择镜像盘目录',
                              );
                              if (dialogCtx.mounted && picked != null && picked.isNotEmpty) {
                                extraPathController.text = picked;
                              }
                            },
                          ),
                        ],
                      ),
                    ],
                  ],
                ],
              ),
            );
          },
        );
      },
    );

    unawaited(Future<void>.delayed(const Duration(milliseconds: 350), () {
      nameController.dispose();
      pathController.dispose();
      extraPathController.dispose();
      quotaController.dispose();
    }));
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return ValueListenableBuilder<bool>(
      valueListenable: widget.bridge.serverRunningNotifier,
      builder: (context, running, _) {
        final totalUsed = _vaults.fold<int>(0, (s, v) => s + v.usedBytes);
        final totalQuota = _vaults.fold<int>(0, (s, v) => s + v.quotaBytes);

        return RefreshIndicator(
          onRefresh: () => _refreshVaults(silent: false),
          child: SingleChildScrollView(
            physics: const AlwaysScrollableScrollPhysics(),
            padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Row(
                  children: <Widget>[
                    if (isWide)
                      Text(
                        '存储',
                        style: Theme.of(context)
                            .textTheme
                            .headlineSmall
                            ?.copyWith(fontWeight: FontWeight.w800),
                      )
                    else
                      Expanded(
                        child: Text(
                          '${SystemEnvHelper.formatBytes(totalUsed)} / ${SystemEnvHelper.formatBytes(totalQuota)}',
                          style:
                              Theme.of(context).textTheme.bodyMedium?.copyWith(
                                    color: neu.textSecondary,
                                    fontWeight: FontWeight.w600,
                                  ),
                        ),
                      ),
                    if (isWide) ...<Widget>[
                      const SizedBox(width: 12),
                      GestureDetector(
                        onTap: running ? null : _ensureServiceRunning,
                        child: NeuStatusBadge(
                          label: _startingService
                              ? '正在启动…'
                              : (running ? '本地服务运行中' : '未启动 · 点击启用'),
                          tone: running
                              ? NeuStatusTone.healthy
                              : NeuStatusTone.warning,
                          showDot: true,
                        ),
                      ),
                      const Spacer(),
                    ],
                    NeuButton(
                      label: '新建存储池',
                      icon: Icons.add_rounded,
                      tone: NeuButtonTone.primary,
                      onPressed: _openCreateVaultDialog,
                    ),
                  ],
                ),
                if (!running && !isWide) ...<Widget>[
                  const SizedBox(height: NeuSpace.md),
                  NeuCard(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 14,
                      vertical: 12,
                    ),
                    backgroundColor: neu.folderGoldSoft,
                    child: Row(
                      children: <Widget>[
                        Icon(
                          Icons.info_outline_rounded,
                          size: 18,
                          color: neu.warning,
                        ),
                        const SizedBox(width: 10),
                        const Expanded(
                          child: Text('本地存储服务尚未启动'),
                        ),
                        NeuButton(
                          label: '一键启用',
                          tone: NeuButtonTone.primary,
                          loading: _startingService,
                          onPressed: _ensureServiceRunning,
                        ),
                      ],
                    ),
                  ),
                ],
                const SizedBox(height: NeuSpace.md),
                if (_loading && _vaults.isEmpty)
                  const Padding(
                    padding: EdgeInsets.symmetric(vertical: 48),
                    child: Center(child: CircularProgressIndicator()),
                  )
                else if (_loadError != null && _vaults.isEmpty)
                  NeuEmptyState(
                    icon: Icons.cloud_off_rounded,
                    title: '加载存储池失败',
                    subtitle: _loadError!,
                    action: NeuButton(
                      label: '重试',
                      icon: Icons.refresh_rounded,
                      tone: NeuButtonTone.primary,
                      onPressed: () => unawaited(_refreshVaults(silent: false)),
                    ),
                  )
                else if (_vaults.isEmpty)
                  NeuEmptyState(
                    icon: Icons.layers_outlined,
                    title: '还没有存储池',
                    subtitle: '新建存储池并指定目录与容量配额，即可接收本机和其他设备的备份数据。',
                    action: NeuButton(
                      label: '新建存储池',
                      icon: Icons.add_rounded,
                      tone: NeuButtonTone.primary,
                      onPressed: _openCreateVaultDialog,
                    ),
                  )
                else ...<Widget>[
                  if (_loadError != null) ...<Widget>[
                    NeuCard(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 14,
                        vertical: 12,
                      ),
                      backgroundColor: neu.folderGoldSoft,
                      child: Row(
                        children: <Widget>[
                          Icon(
                            Icons.warning_amber_rounded,
                            size: 18,
                            color: neu.warning,
                          ),
                          const SizedBox(width: 10),
                          Expanded(child: Text('刷新失败：$_loadError')),
                          NeuButton(
                            label: '重试',
                            onPressed: () =>
                                unawaited(_refreshVaults(silent: false)),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: NeuSpace.md),
                  ],
                  if (isWide)
                    LayoutBuilder(
                      builder: (context, constraints) {
                        final cols = constraints.maxWidth >= 960 ? 3 : 2;
                        final cardWidth =
                            (constraints.maxWidth - NeuSpace.md * (cols - 1)) /
                                cols;
                        return Wrap(
                          spacing: NeuSpace.md,
                          runSpacing: NeuSpace.md,
                          children: <Widget>[
                            for (final vault in _vaults)
                              SizedBox(
                                width: cardWidth,
                                child: _VaultPoolCard(vault: vault),
                              ),
                          ],
                        );
                      },
                    )
                  else
                    Column(
                      children: <Widget>[
                        for (var i = 0; i < _vaults.length; i++) ...<Widget>[
                          _VaultPoolCard(vault: _vaults[i]),
                          if (i < _vaults.length - 1)
                            const SizedBox(height: NeuSpace.md),
                        ],
                      ],
                    ),
                ],
              ],
            ),
          ),
        );
      },
    );
  }
}

class _VaultPoolCard extends StatelessWidget {
  const _VaultPoolCard({required this.vault});

  final VaultSummaryModel vault;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final ratio = vault.quotaBytes > 0
        ? (vault.usedBytes / vault.quotaBytes).clamp(0.0, 1.0)
        : 0.0;
    final mountPath = vault.volumes.isNotEmpty
        ? vault.volumes.first.mountPath
        : '默认存储路径';
    final onlineCount = vault.volumes.where((v) => v.isOnline).length;
    final allOnline =
        vault.volumes.isEmpty || onlineCount == vault.volumes.length;

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              NeuIconBadge(
                icon: Icons.layers_rounded,
                color: neu.accent,
                backgroundColor: neu.accentSoft,
                size: 38,
                iconSize: 20,
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  vault.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                ),
              ),
              NeuStatusBadge(
                label: switch (vault.poolModeValue) {
                  PoolMode.mirrorRaid1 => '双盘镜像',
                  PoolMode.striped => '多盘串联',
                  PoolMode.single => '单盘模式',
                  null => '未知拓扑',
                },
                tone: allOnline ? NeuStatusTone.accent : NeuStatusTone.warning,
              ),
            ],
          ),
          const SizedBox(height: 16),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text(
                '${SystemEnvHelper.formatBytes(vault.usedBytes)} / ${SystemEnvHelper.formatBytes(vault.quotaBytes)}',
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
              ),
              Text(
                '${(ratio * 100).round()}%',
                style: Theme.of(context).textTheme.labelMedium?.copyWith(
                      color: neu.accent,
                      fontWeight: FontWeight.w700,
                    ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          NeuProgressBar(value: ratio, height: 7),
          const SizedBox(height: 12),
          Text(
            mountPath,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
                  color: neu.textSecondary,
                ),
          ),
        ],
      ),
    );
  }
}
