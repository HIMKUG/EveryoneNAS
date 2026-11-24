import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';
import '../snapshots/snapshots_screen.dart';

/// 3. 备份页：
/// - 右上角 `+ 新建备份任务` 按钮（弹窗支持双端目录选择）
/// - 顶部摘要区：左为实时“备份状态”进度卡，右为“增量备份 (FastCDC)”与“备份策略 (GFS 7/4/12)”卡片
/// - 设备组备份与镜像同步规则（v2 Group Rules）：支持差异备份 / 镜像同步、登记本机镜像目录与强制执行前变更预览
/// - 传统备份任务列表：暖金文件夹图标 + 实时启停开关（toggle_sync_rule）+ 立即扫描 + 删除
/// - 底部折叠主/备异盘存储目录配置卡（保留同卷冲突校验）
class BackupScreen extends StatefulWidget {
  const BackupScreen({super.key, required this.bridge, this.subTabNotifier});

  final LanVaultBridge bridge;
  final ValueNotifier<int>? subTabNotifier;

  @override
  State<BackupScreen> createState() => _BackupScreenState();
}

class _BackupScreenState extends State<BackupScreen> {
  int _subTab = 0;
  List<SyncRuleModel> _folders = const <SyncRuleModel>[];
  List<GroupRuleViewModel> _groupRules = const <GroupRuleViewModel>[];
  List<DeviceGroupViewModel> _deviceGroups = const <DeviceGroupViewModel>[];
  List<MirrorRootModel> _mirrorRoots = const <MirrorRootModel>[];
  List<PairedPeerModel> _pairedPeers = const <PairedPeerModel>[];

  final Set<String> _scanningRuleIds = <String>{};
  final Set<String> _togglingRuleIds = <String>{};
  final Set<String> _syncingGroupRuleIds = <String>{};
  final Set<String> _togglingGroupRuleIds = <String>{};
  final Set<String> _previewingGroupRuleIds = <String>{};

  SyncProgressEvent? _latestSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;

  /// 后端在所选目录下自动创建的隔离根（`<所选目录>/everyonenasbackup`）。
  String _primaryRoot = '';
  String? _fallbackRoot;

  bool _loading = true;
  bool _savingTargets = false;
  bool _storageExpanded = false;
  bool _syncPaused = false;
  bool _togglingGlobalPause = false;

  final ScrollController _scrollController = ScrollController();
  final TextEditingController _primaryDirController = TextEditingController();
  final TextEditingController _fallbackDirController = TextEditingController();
  late final Listenable _targetControllersListenable;

  @override
  void initState() {
    super.initState();
    super.initState();
    if (widget.subTabNotifier != null) {
      _subTab = widget.subTabNotifier!.value;
      widget.subTabNotifier!.addListener(_onExternalSubTab);
    }
    _targetControllersListenable = Listenable.merge(<Listenable>[
      _primaryDirController,
      _fallbackDirController,
    ]);
    _loadData();
    _subscribeSync();
  }

  @override
  void dispose() {
    widget.subTabNotifier?.removeListener(_onExternalSubTab);
    _syncSub?.cancel();
    _scrollController.dispose();
    _primaryDirController.dispose();
    _fallbackDirController.dispose();
    super.dispose();
  }

  void _subscribeSync() {
    try {
      _syncSub = widget.bridge.subscribeSyncProgress().listen(
        (event) {
          if (!mounted) return;
          setState(() => _latestSync = event);
        },
        onError: (Object err) {
          if (!mounted) return;
          NeuToast.error(
            context,
            '同步进度监听异常：${LanVaultBridge.formatUserError(err)}',
          );
        },
      );
    } catch (err) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          NeuToast.error(
            context,
            '连接同步事件流失败：${LanVaultBridge.formatUserError(err)}',
          );
        }
      });
    }
  }

  Future<void> _loadData() async {
    setState(() => _loading = true);
    try {
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.listSyncRules(),
        widget.bridge.getBackupStorageConfig(),
        widget.bridge.getSyncPaused(),
        widget.bridge.listGroupRules(),
        widget.bridge.listDeviceGroups(),
        widget.bridge.listMirrorRoots(),
        widget.bridge.refreshPairedPeers(),
      ]);
      if (!mounted) return;
      final rules = results[0] as List<SyncRuleModel>;
      final storage = results[1] as BackupStorageConfigModel;
      final paused = results[2] as bool;
      final groupRules = results[3] as List<GroupRuleViewModel>;
      final deviceGroups = results[4] as List<DeviceGroupViewModel>;
      final mirrorRoots = results[5] as List<MirrorRootModel>;
      final pairedPeers = results[6] as List<PairedPeerModel>;
      setState(() {
        _folders = rules;
        _primaryDirController.text = storage.primaryDir;
        _fallbackDirController.text = storage.fallbackDir ?? '';
        _primaryRoot = storage.primaryRoot;
        _fallbackRoot = storage.fallbackRoot;
        _syncPaused = paused;
        _groupRules = groupRules;
        _deviceGroups = deviceGroups;
        _mirrorRoots = mirrorRoots;
        _pairedPeers = pairedPeers;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(
        context,
        '加载备份配置失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _openCreateTaskDialog() async {
    final created = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) => _CreateBackupTaskDialog(
        bridge: widget.bridge,
        existingRules: _folders,
      ),
    );
    if (created == true && mounted) {
      await _loadData();
    }
  }

  Future<void> _openRegisterMirrorRootDialog() async {
    final pathController = TextEditingController();
    final nameController = TextEditingController();
    var submitting = false;

    final registered = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) {
        return StatefulBuilder(
          builder: (context, setDialogState) {
            return NeuDialog(
              title: '登记本机镜像目录',
              subtitle: '镜像同步仅允许写入已显式登记的本机目标目录，不支持符号链接。',
              maxWidth: 480,
              actions: <Widget>[
                NeuButton(
                  label: '取消',
                  semanticLabel: '取消登记镜像目录',
                  onPressed: submitting
                      ? null
                      : () => Navigator.of(dialogContext).pop(false),
                ),
                NeuButton(
                  label: '登记镜像目录',
                  icon: Icons.verified_outlined,
                  tone: NeuButtonTone.primary,
                  loading: submitting,
                  semanticLabel: '确认登记本机镜像目录',
                  onPressed: () async {
                    final path = pathController.text.trim();
                    if (path.isEmpty) {
                      NeuToast.warning(dialogContext, '请先选择或输入本机目录路径');
                      return;
                    }
                    setDialogState(() => submitting = true);
                    try {
                      await widget.bridge.registerMirrorRoot(
                        canonicalPath: path,
                        displayName: nameController.text.trim(),
                      );
                      if (!dialogContext.mounted) return;
                      Navigator.of(dialogContext).pop(true);
                    } catch (err) {
                      if (!dialogContext.mounted) return;
                      setDialogState(() => submitting = false);
                      NeuToast.error(
                        dialogContext,
                        '登记失败：${LanVaultBridge.formatUserError(err)}',
                      );
                    }
                  },
                ),
              ],
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: <Widget>[
                      Expanded(
                        child: NeuInsetField(
                          controller: pathController,
                          label: '本机目标目录路径',
                          hint: Platform.isAndroid
                              ? '/storage/emulated/0/LanVaultMirror'
                              : r'D:\LanVaultMirror',
                          prefixIcon: Icons.folder_special_outlined,
                        ),
                      ),
                      const SizedBox(width: NeuSpace.sm),
                      NeuButton(
                        label: '浏览',
                        icon: Icons.folder_open_rounded,
                        semanticLabel: '浏览选择本机镜像目录',
                        onPressed: () async {
                          final picked = await widget.bridge.pickDirectory(
                            description: '选择要登记为镜像目标的本机文件夹',
                          );
                          if (picked != null && dialogContext.mounted) {
                            setDialogState(() => pathController.text = picked);
                          }
                        },
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  NeuInsetField(
                    controller: nameController,
                    label: '显示名称（可选）',
                    hint: '例如：工作站镜像盘',
                    prefixIcon: Icons.label_outline_rounded,
                  ),
                  if (_mirrorRoots
                      .where((r) => r.isActive)
                      .isNotEmpty) ...<Widget>[
                    const SizedBox(height: 16),
                    Text(
                      '已登记的镜像目录',
                      style: Theme.of(dialogContext).textTheme.titleSmall,
                    ),
                    const SizedBox(height: 8),
                    for (final root
                        in _mirrorRoots.where((r) => r.isActive)) ...<Widget>[
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 4),
                        child: Row(
                          children: <Widget>[
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: <Widget>[
                                  Text(
                                    root.label,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: Theme.of(dialogContext)
                                        .textTheme
                                        .bodyMedium,
                                  ),
                                  Text(
                                    root.canonicalRoot,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: Theme.of(dialogContext)
                                        .textTheme
                                        .bodySmall,
                                  ),
                                ],
                              ),
                            ),
                            const SizedBox(width: 8),
                            NeuButton(
                              label: '撤销',
                              tone: NeuButtonTone.danger,
                              semanticLabel: '撤销镜像目录 ${root.label}',
                              onPressed: () async {
                                try {
                                  await widget.bridge
                                      .revokeMirrorRoot(root.mirrorRootId);
                                  if (!dialogContext.mounted) return;
                                  NeuToast.info(
                                    dialogContext,
                                    '已撤销镜像目录「${root.label}」',
                                  );
                                  Navigator.of(dialogContext).pop(true);
                                } catch (err) {
                                  if (!dialogContext.mounted) return;
                                  NeuToast.error(
                                    dialogContext,
                                    '撤销失败：${LanVaultBridge.formatUserError(err)}',
                                  );
                                }
                              },
                            ),
                          ],
                        ),
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

    pathController.dispose();
    nameController.dispose();
    if (registered == true && mounted) {
      NeuToast.success(context, '镜像目录列表已更新');
      await _loadData();
    }
  }

  Future<void> _openGroupRuleDialog({GroupRuleViewModel? editing}) async {
    if (_deviceGroups.isEmpty) {
      NeuToast.warning(context, '请先在“设备”页创建至少一个设备组');
      return;
    }

    final saved = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) => _GroupRuleEditorDialog(
        bridge: widget.bridge,
        deviceGroups: _deviceGroups,
        mirrorRoots: _mirrorRoots,
        pairedPeers: _pairedPeers,
        editing: editing,
      ),
    );
    if (saved == true && mounted) {
      await _loadData();
    }
  }

  Future<bool> _showMirrorPreviewDialog(
    GroupSyncRuleModel rule, {
    required bool requireConfirmToEnable,
  }) async {
    if (_previewingGroupRuleIds.contains(rule.ruleId)) return false;
    setState(() => _previewingGroupRuleIds.add(rule.ruleId));
    List<MirrorPreviewModel> previews;
    try {
      previews = await widget.bridge.previewGroupRule(rule.ruleId);
    } catch (err) {
      if (mounted) {
        setState(() => _previewingGroupRuleIds.remove(rule.ruleId));
        NeuToast.error(
          context,
          '生成变更预览失败：${LanVaultBridge.formatUserError(err)}',
        );
      }
      return false;
    }
    if (!mounted) return false;
    setState(() => _previewingGroupRuleIds.remove(rule.ruleId));

    final confirmed = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) => _MirrorPreviewDialog(
        ruleName: rule.name,
        isMirrorMode: rule.isMirrorSync,
        previews: previews,
        pairedPeers: _pairedPeers,
        requireConfirmToEnable: requireConfirmToEnable,
      ),
    );
    return confirmed == true;
  }

  Future<void> _toggleGroupRule(
    GroupRuleViewModel view,
    bool nextEnabled,
  ) async {
    final rule = view.rule;
    if (_togglingGroupRuleIds.contains(rule.ruleId)) return;

    // 启用镜像同步规则前必须先执行并确认变更预览。
    if (nextEnabled && rule.isMirrorSync) {
      final confirmed = await _showMirrorPreviewDialog(
        rule,
        requireConfirmToEnable: true,
      );
      if (!confirmed || !mounted) return;
    }

    setState(() => _togglingGroupRuleIds.add(rule.ruleId));
    try {
      await widget.bridge.toggleGroupRule(
        ruleId: rule.ruleId,
        enabled: nextEnabled,
      );
      if (!mounted) return;
      setState(() => _togglingGroupRuleIds.remove(rule.ruleId));
      NeuToast.success(
        context,
        '组规则「${rule.name}」已${nextEnabled ? '启用' : '停用'}',
      );
      await _loadData();
    } catch (err) {
      if (!mounted) return;
      setState(() => _togglingGroupRuleIds.remove(rule.ruleId));
      NeuToast.error(
        context,
        '切换组规则状态失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _triggerGroupRuleSync(GroupRuleViewModel view) async {
    final rule = view.rule;
    if (_syncingGroupRuleIds.contains(rule.ruleId)) return;
    setState(() => _syncingGroupRuleIds.add(rule.ruleId));
    try {
      final outcome = await widget.bridge.triggerGroupRuleSync(rule.ruleId);
      if (!mounted) return;
      setState(() => _syncingGroupRuleIds.remove(rule.ruleId));
      final okCount = outcome.bindings
          .where((b) => b.status == 'synced' || b.status == 'ok')
          .length;
      NeuToast.success(
        context,
        '规则「${rule.name}」已扫描 ${outcome.scannedEntries} 项，$okCount/${outcome.bindings.length} 个目标同步完成',
      );
      await _loadData();
    } catch (err) {
      if (!mounted) return;
      setState(() => _syncingGroupRuleIds.remove(rule.ruleId));
      NeuToast.error(
        context,
        '触发组规则同步失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _deleteGroupRule(GroupRuleViewModel view) async {
    try {
      await widget.bridge.deleteGroupRule(view.rule.ruleId);
      if (!mounted) return;
      NeuToast.info(context, '已删除组规则「${view.rule.name}」');
      await _loadData();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '删除组规则失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  String _resolveDeviceName(String deviceId) {
    for (final peer in _pairedPeers) {
      if (peer.deviceId == deviceId) {
        return peer.deviceName.trim().isEmpty
            ? peer.deviceId
            : peer.deviceName.trim();
      }
    }
    return deviceId;
  }

  (String, NeuStatusTone) _bindingStatusMeta(RuleTargetBindingModel binding) {
    if (!binding.enabled || binding.isRevoked) {
      return ('已停用', NeuStatusTone.neutral);
    }
    if (binding.needsBindingConfirm) {
      return ('待确认绑定', NeuStatusTone.warning);
    }
    return switch (binding.status.toLowerCase()) {
      'synced' || 'ok' || 'up_to_date' => ('已同步', NeuStatusTone.healthy),
      'blocked' || 'conflict' || 'error' => ('阻塞', NeuStatusTone.danger),
      'disabled' => ('已停用', NeuStatusTone.neutral),
      'needs_confirm' || 'pending_confirm' => ('待确认绑定', NeuStatusTone.warning),
      _ => ('离线待同步', NeuStatusTone.accent),
    };
  }

  Future<void> _toggleGlobalPause() async {
    if (_togglingGlobalPause) return;
    setState(() => _togglingGlobalPause = true);
    try {
      final applied = await widget.bridge.setSyncPaused(!_syncPaused);
      if (!mounted) return;
      setState(() {
        _syncPaused = applied;
        _togglingGlobalPause = false;
      });
      NeuToast.info(context, applied ? '已全局暂停自动备份' : '已恢复自动备份监听');
    } catch (err) {
      if (!mounted) return;
      setState(() => _togglingGlobalPause = false);
      NeuToast.error(
        context,
        '切换全局暂停失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _changeRuleSchedule(
    SyncRuleModel folder,
    String scheduleMode,
  ) async {
    try {
      final applied = await widget.bridge.setSyncRuleSchedule(
        ruleId: folder.ruleId,
        scheduleMode: scheduleMode,
      );
      if (!mounted) return;
      setState(() {
        _folders = _folders
            .map(
              (r) => r.ruleId == folder.ruleId
                  ? r.copyWith(scheduleMode: applied)
                  : r,
            )
            .toList(growable: false);
      });
      NeuToast.success(context, '已更新调度策略');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '更新调度策略失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _toggleRule(SyncRuleModel folder, bool nextEnabled) async {
    if (_togglingRuleIds.contains(folder.ruleId)) return;
    setState(() => _togglingRuleIds.add(folder.ruleId));
    try {
      final applied = await widget.bridge.toggleSyncRule(
        ruleId: folder.ruleId,
        enabled: nextEnabled,
      );
      if (!mounted) return;
      setState(() {
        _togglingRuleIds.remove(folder.ruleId);
        _folders = _folders
            .map(
              (r) =>
                  r.ruleId == folder.ruleId ? r.copyWith(enabled: applied) : r,
            )
            .toList(growable: false);
      });
      NeuToast.success(
        context,
        '${SystemEnvHelper.folderDisplayName(folder.localDir)} 已${applied ? '启用' : '暂停'}',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _togglingRuleIds.remove(folder.ruleId));
      NeuToast.error(
        context,
        '切换任务状态失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _triggerRuleScan(SyncRuleModel folder) async {
    if (_scanningRuleIds.contains(folder.ruleId)) return;
    setState(() => _scanningRuleIds.add(folder.ruleId));
    try {
      await widget.bridge.triggerIncrementalScan(folder.ruleId);
      if (!mounted) return;
      setState(() => _scanningRuleIds.remove(folder.ruleId));
      NeuToast.success(
        context,
        '已触发 ${SystemEnvHelper.folderDisplayName(folder.localDir)} 增量扫描',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _scanningRuleIds.remove(folder.ruleId));
      NeuToast.error(
        context,
        '触发扫描失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _removeFolder(SyncRuleModel folder) async {
    try {
      await widget.bridge.deleteSyncRule(folder.ruleId);
      final updated = await widget.bridge.listSyncRules();
      if (!mounted) return;
      setState(() => _folders = updated);
      NeuToast.info(
        context,
        '已移除 ${SystemEnvHelper.folderDisplayName(folder.localDir)}',
      );
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '移除失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _pickTargetDirectory({required bool isPrimary}) async {
    try {
      final picked = await widget.bridge.pickDirectory(
        description: isPrimary ? '选择主备份文件夹' : '选择备用备份文件夹（需与主备份位于不同存储卷）',
      );
      if (picked == null || !mounted) return;

      if (!isPrimary) {
        final primaryDrive =
            SystemEnvHelper.extractDriveKey(_primaryDirController.text);
        final fallbackDrive = SystemEnvHelper.extractDriveKey(picked);
        if (primaryDrive != null &&
            fallbackDrive != null &&
            primaryDrive == fallbackDrive) {
          NeuToast.warning(
            context,
            '备用备份文件夹不能与主备份在同一存储卷（当前同为 $primaryDrive）',
          );
          return;
        }
      }

      if (isPrimary) {
        _primaryDirController.text = picked;
      } else {
        _fallbackDirController.text = picked;
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '打开文件夹选择器失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _saveBackupTargets() async {
    final primary = _primaryDirController.text.trim();
    final fallback = _fallbackDirController.text.trim();

    if (primary.isEmpty) {
      NeuToast.warning(context, '请先选择主备份文件夹');
      return;
    }

    if (fallback.isNotEmpty) {
      final primaryDrive = SystemEnvHelper.extractDriveKey(primary);
      final fallbackDrive = SystemEnvHelper.extractDriveKey(fallback);
      if (primaryDrive != null &&
          fallbackDrive != null &&
          primaryDrive == fallbackDrive) {
        NeuToast.warning(
          context,
          '主备份与备用备份不能位于同一存储卷（同为 $primaryDrive）',
        );
        return;
      }
    }

    setState(() => _savingTargets = true);
    try {
      final applied = await widget.bridge.setBackupStorageConfig(
        primaryDir: primary,
        fallbackDir: fallback.isEmpty ? null : fallback,
      );
      if (!mounted) return;
      setState(() {
        _savingTargets = false;
        _primaryRoot = applied.primaryRoot;
        _fallbackRoot = applied.fallbackRoot;
      });
      NeuToast.success(
        context,
        applied.primaryRoot.isEmpty
            ? '备份存储位置已保存'
            : '备份存储位置已保存，数据将写入 ${SystemEnvHelper.folderDisplayName(applied.primaryRoot)} 隔离子目录',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _savingTargets = false);
      NeuToast.error(context, '保存失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  String _syncTitle(SyncProgressEvent? sync) {
    if (sync == null || sync.status.isEmpty) return '等待备份任务';
    return switch (sync.status.toLowerCase()) {
      'idle' => '空闲待命',
      'up_to_date' => '所有目录已是最新',
      'completed' => '增量备份已完成',
      'scanning' => '正在扫描变更…',
      'syncing' || 'uploading' => '正在备份…',
      _ => sync.status,
    };
  }

  void _onExternalSubTab() {
    if (!mounted || widget.subTabNotifier == null) return;
    final next = widget.subTabNotifier!.value.clamp(0, 1);
    if (next != _subTab) {
      setState(() => _subTab = next);
    }
  }

  @override
  Widget build(BuildContext context) {
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        Padding(
          padding: EdgeInsets.fromLTRB(
            isWide ? NeuSpace.lg : NeuSpace.md,
            isWide ? NeuSpace.lg : NeuSpace.sm,
            isWide ? NeuSpace.lg : NeuSpace.md,
            NeuSpace.sm,
          ),
          child: Row(
            children: <Widget>[
              if (isWide) ...<Widget>[
                Text(
                  '备份',
                  style: Theme.of(context)
                      .textTheme
                      .headlineSmall
                      ?.copyWith(fontWeight: FontWeight.w800),
                ),
                const SizedBox(width: 20),
              ],
              isWide
                  ? SizedBox(
                      width: 240,
                      child: NeuSegmentedControl<int>(
                        semanticLabel: '备份视角切换',
                        value: _subTab,
                        onChanged: (v) {
                          setState(() => _subTab = v);
                          widget.subTabNotifier?.value = v;
                        },
                        segments: const <NeuSegment<int>>[
                          NeuSegment<int>(value: 0, label: '备份任务'),
                          NeuSegment<int>(value: 1, label: '历史版本'),
                        ],
                      ),
                    )
                  : Expanded(
                      child: NeuSegmentedControl<int>(
                        semanticLabel: '备份视角切换',
                        value: _subTab,
                        onChanged: (v) {
                          setState(() => _subTab = v);
                          widget.subTabNotifier?.value = v;
                        },
                        segments: const <NeuSegment<int>>[
                          NeuSegment<int>(value: 0, label: '备份任务'),
                          NeuSegment<int>(value: 1, label: '历史版本'),
                        ],
                      ),
                    ),
              if (isWide) const Spacer(),
            ],
          ),
        ),
        Expanded(
          child: _subTab == 1
              ? SnapshotsScreen(bridge: widget.bridge)
              : _buildTasksTab(context, isWide),
        ),
      ],
    );
  }

  Widget _buildTasksTab(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;
    final sync = _latestSync;
    final dedupPct = ((sync?.dedupRatio ?? 0.0) * 100).round();

    final list = ListView(
      controller: _scrollController,
      padding: EdgeInsets.fromLTRB(
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : 104,
      ),
      children: <Widget>[
        if (isWide)
          Row(
            children: <Widget>[
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text('备份', style: textTheme.headlineSmall),
                    const SizedBox(height: 4),
                    Text(
                      '管理设备组同步规则、镜像目录与传统备份任务',
                      style: textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
              const SizedBox(width: NeuSpace.sm),
              NeuButton(
                label: '新建备份任务',
                icon: Icons.add_rounded,
                tone: NeuButtonTone.primary,
                onPressed: _openCreateTaskDialog,
              ),
            ],
          ),
        const SizedBox(height: 18),
        if (isWide)
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Expanded(flex: 12, child: _buildBackupStatusCard(context, sync)),
              const SizedBox(width: NeuSpace.md),
              Expanded(
                flex: 8,
                child: Column(
                  children: <Widget>[
                    _buildFeatureMiniCard(
                      context,
                      icon: Icons.grid_view_rounded,
                      title: '增量备份 (FastCDC)',
                      subtitle: sync != null && sync.totalChunks > 0
                          ? '实时去重率 $dedupPct% · 跳过 ${sync.chunksDeduplicated} 块'
                          : '动态分块去重 · 等待首次扫描统计',
                    ),
                    const SizedBox(height: 12),
                    _buildFeatureMiniCard(
                      context,
                      icon: Icons.event_repeat_rounded,
                      title: '备份策略 (GFS 7/4/12)',
                      subtitle: '自动保留 7 日 / 4 周 / 12 月历史快照',
                    ),
                  ],
                ),
              ),
            ],
          )
        else ...<Widget>[
          _buildBackupStatusCard(context, sync),
          const SizedBox(height: 12),
          _buildFeatureMiniCard(
            context,
            icon: Icons.grid_view_rounded,
            title: '增量备份 (FastCDC)',
            subtitle: sync != null && sync.totalChunks > 0
                ? '实时去重率 $dedupPct% · 跳过 ${sync.chunksDeduplicated} 块'
                : '动态分块去重 · GFS 保留 7/4/12',
          ),
        ],
        const SizedBox(height: NeuSpace.md),
        _buildTaskListCard(context, isWide),
        const SizedBox(height: NeuSpace.md),
        _buildGroupRulesSection(context, isWide),
        const SizedBox(height: NeuSpace.md),
        _buildStorageConfigCard(context, isWide),
      ],
    );
    if (isWide) return list;
    return Stack(
      children: <Widget>[
        list,
        Positioned(
          right: 18,
          bottom: 18,
          child: _TactileCircleFab(
            onPressed: _openCreateTaskDialog,
            semanticLabel: '新建备份任务',
          ),
        ),
      ],
    );
  }

  Widget _buildGroupRulesSection(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;
    final activeMirrorCount = _mirrorRoots.where((r) => r.isActive).length;

    return Semantics(
      container: true,
      label: '设备组备份与镜像同步规则',
      child: NeuCard(
        padding: const EdgeInsets.all(20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Wrap(
              spacing: NeuSpace.sm,
              runSpacing: NeuSpace.sm,
              alignment: WrapAlignment.spaceBetween,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: <Widget>[
                Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Wrap(
                      spacing: 8,
                      runSpacing: 4,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: <Widget>[
                        Text(
                          '设备组备份与镜像同步规则 (${_groupRules.length})',
                          style: textTheme.titleMedium,
                        ),
                        NeuStatusBadge(
                          label: '已登记镜像根 $activeMirrorCount',
                          tone: NeuStatusTone.neutral,
                          showDot: false,
                        ),
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '按设备组配置差异备份或单向镜像同步，启用镜像前强制预览变更',
                      style: textTheme.bodySmall,
                    ),
                  ],
                ),
                Wrap(
                  spacing: NeuSpace.sm,
                  runSpacing: NeuSpace.xs,
                  children: <Widget>[
                    NeuButton(
                      label: '登记本机镜像目录',
                      icon: Icons.folder_special_outlined,
                      semanticLabel: '登记本机镜像目录',
                      onPressed: _openRegisterMirrorRootDialog,
                    ),
                    NeuButton(
                      label: '新建组规则',
                      icon: Icons.add_circle_outline_rounded,
                      tone: NeuButtonTone.primary,
                      semanticLabel: '新建设备组备份或镜像同步规则',
                      onPressed: () => _openGroupRuleDialog(),
                    ),
                  ],
                ),
              ],
            ),
            const SizedBox(height: 12),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(10)),
                border: Border.all(color: NeuTokens.hairlineBorder),
              ),
              child: Text(
                '镜像同步范围说明：仅同步常规文件、空目录及平台可表示的修改时间（mtime）；符号链接（symlink）与特殊文件不支持同步。',
                style: textTheme.bodySmall,
              ),
            ),
            const SizedBox(height: 14),
            if (_loading && _groupRules.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: Text('正在加载设备组规则…', style: textTheme.bodySmall),
              )
            else if (_groupRules.isEmpty)
              const NeuEmptyState(
                icon: Icons.hub_outlined,
                title: '暂无设备组同步规则',
                subtitle: '点击“新建组规则”选择设备组、源目录及差异备份 / 镜像同步模式。',
              )
            else
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  for (var i = 0; i < _groupRules.length; i++) ...<Widget>[
                    _buildGroupRuleItem(context, _groupRules[i]),
                    if (i < _groupRules.length - 1) const SizedBox(height: 12),
                  ],
                ],
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildGroupRuleItem(BuildContext context, GroupRuleViewModel view) {
    final textTheme = Theme.of(context).textTheme;
    final rule = view.rule;
    final isSyncing = _syncingGroupRuleIds.contains(rule.ruleId);
    final isToggling = _togglingGroupRuleIds.contains(rule.ruleId);
    final isPreviewing = _previewingGroupRuleIds.contains(rule.ruleId);

    final srcName = _resolveDeviceName(rule.sourceDeviceId);
    final targetNames = view.bindings.isEmpty
        ? '未绑定目标'
        : view.bindings
            .map((b) => _resolveDeviceName(b.targetDeviceId))
            .join(', ');

    return Semantics(
      container: true,
      label: '组规则 ${rule.name}，${rule.modeLabel}，$srcName 到 $targetNames',
      child: NeuCard(
        key: ValueKey<String>('group_rule_${rule.ruleId}'),
        inset: true,
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                NeuIconBadge(
                  icon: rule.isMirrorSync
                      ? Icons.compare_arrows_rounded
                      : Icons.backup_rounded,
                  color:
                      rule.isMirrorSync ? NeuTokens.warning : NeuTokens.accent,
                  backgroundColor: rule.isMirrorSync
                      ? NeuTokens.folderGoldSoft
                      : NeuTokens.accentSoft,
                  size: 40,
                  iconSize: 20,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Wrap(
                        spacing: 8,
                        runSpacing: 4,
                        crossAxisAlignment: WrapCrossAlignment.center,
                        children: <Widget>[
                          Text(rule.name, style: textTheme.titleSmall),
                          NeuStatusBadge(
                            label: rule.modeLabel,
                            tone: rule.isMirrorSync
                                ? NeuStatusTone.warning
                                : NeuStatusTone.accent,
                            showDot: false,
                          ),
                          if (rule.isMirrorSync)
                            NeuStatusBadge(
                              label: '归档保留 ${rule.archiveRetentionDays} 天',
                              tone: NeuStatusTone.neutral,
                              showDot: false,
                            ),
                        ],
                      ),
                      const SizedBox(height: 3),
                      Text(
                        '$srcName -> $targetNames',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: textTheme.bodyMedium,
                      ),
                      const SizedBox(height: 2),
                      Text(
                        '源目录：${rule.sourceRoot}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 8),
                NeuSwitch(
                  title: '',
                  semanticLabel: '切换组规则 ${rule.name} 启用状态',
                  value: rule.enabled,
                  onChanged:
                      isToggling ? null : (val) => _toggleGroupRule(view, val),
                ),
              ],
            ),
            if (view.bindings.isNotEmpty) ...<Widget>[
              const SizedBox(height: 10),
              Wrap(
                spacing: 8,
                runSpacing: 6,
                children: <Widget>[
                  for (final binding in view.bindings)
                    Builder(
                      builder: (context) {
                        final (label, tone) = _bindingStatusMeta(binding);
                        final tName =
                            _resolveDeviceName(binding.targetDeviceId);
                        return NeuStatusBadge(
                          label: '$tName · $label',
                          tone: tone,
                        );
                      },
                    ),
                ],
              ),
            ],
            const SizedBox(height: 12),
            Wrap(
              spacing: NeuSpace.xs,
              runSpacing: NeuSpace.xs,
              children: <Widget>[
                NeuButton(
                  label: '变更预览',
                  icon: Icons.preview_rounded,
                  loading: isPreviewing,
                  semanticLabel: '预览组规则 ${rule.name} 变更',
                  onPressed: () => _showMirrorPreviewDialog(
                    rule,
                    requireConfirmToEnable: false,
                  ),
                ),
                NeuButton(
                  label: '立即同步',
                  icon: Icons.sync_rounded,
                  tone: NeuButtonTone.primary,
                  loading: isSyncing,
                  semanticLabel: '立即执行组规则 ${rule.name} 同步',
                  onPressed:
                      rule.enabled ? () => _triggerGroupRuleSync(view) : null,
                ),
                NeuButton(
                  label: '编辑',
                  icon: Icons.edit_outlined,
                  semanticLabel: '编辑组规则 ${rule.name}',
                  onPressed: () => _openGroupRuleDialog(editing: view),
                ),
                NeuButton(
                  label: '删除',
                  icon: Icons.delete_outline_rounded,
                  tone: NeuButtonTone.danger,
                  semanticLabel: '删除组规则 ${rule.name}',
                  onPressed: () => _deleteGroupRule(view),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildBackupStatusCard(BuildContext context, SyncProgressEvent? sync) {
    final textTheme = Theme.of(context).textTheme;
    final isActive = sync != null &&
        (sync.status.toLowerCase().startsWith('sync') ||
            sync.status.toLowerCase().startsWith('upload') ||
            sync.status.toLowerCase() == 'scanning');

    final String processedStr;
    if (sync == null) {
      processedStr = '0 B / 0 B';
    } else if (sync.totalBytes <= 0) {
      processedStr = SystemEnvHelper.formatBytes(sync.bytesProcessed);
    } else {
      processedStr =
          '${SystemEnvHelper.formatBytes(sync.bytesProcessed)} / ${SystemEnvHelper.formatBytes(sync.totalBytes)}';
    }
    final ratio = sync?.progressRatio ?? 0.0;
    final speedLabel = (sync != null && sync.throughputBps > 0)
        ? '${SystemEnvHelper.formatBytes(sync.throughputBps)}/s'
        : (isActive ? '测速中…' : null);

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Container(
                width: 40,
                height: 40,
                decoration: const BoxDecoration(
                  color: NeuTokens.accentSoft,
                  borderRadius: BorderRadius.all(Radius.circular(12)),
                ),
                child: const Icon(
                  Icons.cloud_upload_rounded,
                  size: 21,
                  color: NeuTokens.accent,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text('备份状态', style: textTheme.titleMedium),
                    const SizedBox(height: 2),
                    Text(_syncTitle(sync), style: textTheme.bodySmall),
                  ],
                ),
              ),
              if (speedLabel != null) ...<Widget>[
                NeuStatusBadge(
                  label: speedLabel,
                  tone: NeuStatusTone.accent,
                ),
                const SizedBox(width: 6),
              ],
              IconButton(
                tooltip: _syncPaused ? '恢复自动备份' : '暂停全部备份',
                icon: Icon(
                  _syncPaused ? Icons.play_arrow_rounded : Icons.pause_rounded,
                  color: _syncPaused ? NeuTokens.warning : NeuTokens.accent,
                ),
                onPressed: _togglingGlobalPause ? null : _toggleGlobalPause,
              ),
            ],
          ),
          const SizedBox(height: 16),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text(
                processedStr,
                style: textTheme.labelMedium?.copyWith(
                  color: NeuTokens.textPrimary,
                  fontFeatures: const <FontFeature>[
                    FontFeature.tabularFigures(),
                  ],
                ),
              ),
              if (sync != null && sync.filePath.isNotEmpty)
                Flexible(
                  child: Text(
                    SystemEnvHelper.folderDisplayName(sync.filePath),
                    overflow: TextOverflow.ellipsis,
                    style: textTheme.bodySmall,
                  ),
                ),
            ],
          ),
          const SizedBox(height: 10),
          NeuProgressBar(
            value: ratio,
            height: 8,
            semanticLabel: '备份实时进度',
          ),
        ],
      ),
    );
  }

  Widget _buildFeatureMiniCard(
    BuildContext context, {
    required IconData icon,
    required String title,
    required String subtitle,
  }) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
      child: Row(
        children: <Widget>[
          Container(
            width: 38,
            height: 38,
            decoration: const BoxDecoration(
              color: NeuTokens.accentSoft,
              borderRadius: BorderRadius.all(Radius.circular(10)),
            ),
            child: Icon(icon, size: 19, color: NeuTokens.accent),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(title, style: textTheme.titleSmall),
                const SizedBox(height: 2),
                Text(
                  subtitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: textTheme.bodySmall,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildTaskListCard(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: <Widget>[
              Text('备份任务 (${_folders.length})', style: textTheme.titleMedium),
              NeuButton(
                label: '刷新',
                icon: Icons.refresh_rounded,
                onPressed: _loadData,
              ),
            ],
          ),
          const SizedBox(height: 14),
          if (_loading)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 16),
              child: Text('正在加载备份任务…', style: textTheme.bodySmall),
            )
          else if (_folders.isEmpty)
            const NeuEmptyState(
              icon: Icons.folder_copy_outlined,
              title: '尚未添加备份目录',
              subtitle: '点击“新建备份任务”选择需要保护的本地文件夹。',
            )
          else
            for (var i = 0; i < _folders.length; i++) ...<Widget>[
              Builder(
                key: ValueKey<String>(_folders[i].ruleId),
                builder: (context) {
                  final folder = _folders[i];
                  final isScanning = _scanningRuleIds.contains(folder.ruleId);
                  final isToggling = _togglingRuleIds.contains(folder.ruleId);
                  final displayName = SystemEnvHelper.folderDisplayName(
                    folder.localDir,
                  );

                  return Padding(
                    padding: const EdgeInsets.symmetric(vertical: 10),
                    child: Row(
                      children: <Widget>[
                        const NeuIconBadge(
                          icon: Icons.folder_rounded,
                          color: NeuTokens.folderGold,
                          backgroundColor: NeuTokens.folderGoldSoft,
                          size: 42,
                          iconSize: 22,
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                displayName,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.titleSmall?.copyWith(
                                  fontSize: 15,
                                ),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                isWide
                                    ? folder.localDir
                                    : '${folder.scheduleLabel} · ${folder.localDir}',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        if (isWide) ...<Widget>[
                          const SizedBox(width: 8),
                          NeuStatusBadge(
                            label: folder.scheduleLabel,
                            tone: folder.scheduleMode == 'manual'
                                ? NeuStatusTone.neutral
                                : NeuStatusTone.accent,
                            showDot: false,
                          ),
                        ],
                        const SizedBox(width: 8),
                        NeuSwitch(
                          title: '',
                          semanticLabel: '切换 $displayName 自动备份',
                          value: folder.enabled,
                          onChanged: isToggling
                              ? null
                              : (val) => _toggleRule(folder, val),
                        ),
                        if (isWide) ...<Widget>[
                          const SizedBox(width: 4),
                          IconButton(
                            tooltip: '立即增量扫描',
                            icon: isScanning
                                ? const SizedBox(
                                    width: 18,
                                    height: 18,
                                    child: CircularProgressIndicator(
                                      strokeWidth: 2,
                                    ),
                                  )
                                : const Icon(
                                    Icons.play_arrow_rounded,
                                    color: NeuTokens.accent,
                                  ),
                            onPressed: isScanning
                                ? null
                                : () => _triggerRuleScan(folder),
                          ),
                        ],
                        PopupMenuButton<String>(
                          tooltip: '更多操作',
                          icon: isScanning && !isWide
                              ? const SizedBox(
                                  width: 18,
                                  height: 18,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                  ),
                                )
                              : const Icon(
                                  Icons.more_horiz_rounded,
                                  color: NeuTokens.textSecondary,
                                ),
                          onSelected: (value) {
                            if (value == 'scan') {
                              _triggerRuleScan(folder);
                            } else if (value == 'sched_realtime') {
                              _changeRuleSchedule(folder, 'realtime');
                            } else if (value == 'sched_daily') {
                              _changeRuleSchedule(folder, 'daily');
                            } else if (value == 'sched_manual') {
                              _changeRuleSchedule(folder, 'manual');
                            } else if (value == 'delete') {
                              _removeFolder(folder);
                            }
                          },
                          itemBuilder: (context) =>
                              const <PopupMenuEntry<String>>[
                            PopupMenuItem<String>(
                              value: 'scan',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.sync_rounded,
                                    size: 18,
                                    color: NeuTokens.accent,
                                  ),
                                  SizedBox(width: 8),
                                  Text('立即增量扫描'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'sched_realtime',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.bolt_rounded,
                                    size: 18,
                                    color: NeuTokens.accent,
                                  ),
                                  SizedBox(width: 8),
                                  Text('设为：实时 · 增量'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'sched_daily',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.today_rounded,
                                    size: 18,
                                    color: NeuTokens.accent,
                                  ),
                                  SizedBox(width: 8),
                                  Text('设为：每日 · 增量'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'sched_manual',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.touch_app_outlined,
                                    size: 18,
                                    color: NeuTokens.textSecondary,
                                  ),
                                  SizedBox(width: 8),
                                  Text('设为：手动'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'delete',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.delete_outline_rounded,
                                    size: 18,
                                    color: NeuTokens.danger,
                                  ),
                                  SizedBox(width: 8),
                                  Text(
                                    '删除备份任务',
                                    style: TextStyle(color: NeuTokens.danger),
                                  ),
                                ],
                              ),
                            ),
                          ],
                        ),
                      ],
                    ),
                  );
                },
              ),
              if (i < _folders.length - 1) const Divider(height: 1),
            ],
        ],
      ),
    );
  }

  Widget _buildStorageConfigCard(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;

    return AnimatedBuilder(
      animation: _targetControllersListenable,
      builder: (context, _) {
        final primaryDrive =
            SystemEnvHelper.extractDriveKey(_primaryDirController.text);
        final fallbackDrive =
            SystemEnvHelper.extractDriveKey(_fallbackDirController.text);
        final sameDriveConflict = primaryDrive != null &&
            fallbackDrive != null &&
            primaryDrive == fallbackDrive;

        return NeuCard(
          padding: const EdgeInsets.all(18),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              InkWell(
                onTap: () {
                  final next = !_storageExpanded;
                  setState(() => _storageExpanded = next);
                  if (next) {
                    WidgetsBinding.instance.addPostFrameCallback((_) {
                      if (_scrollController.hasClients) {
                        _scrollController.animateTo(
                          _scrollController.position.maxScrollExtent,
                          duration: const Duration(milliseconds: 280),
                          curve: Curves.linearToEaseOut,
                        );
                      }
                    });
                  }
                },
                borderRadius: const BorderRadius.all(Radius.circular(10)),
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 4),
                  child: Row(
                    children: <Widget>[
                      Container(
                        width: 36,
                        height: 36,
                        decoration: const BoxDecoration(
                          color: NeuTokens.insetSurface,
                          borderRadius: BorderRadius.all(Radius.circular(10)),
                        ),
                        child: const Icon(
                          Icons.storage_rounded,
                          size: 18,
                          color: NeuTokens.textPrimary,
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text('主 / 备异盘存储目录配置', style: textTheme.titleSmall),
                            const SizedBox(height: 2),
                            Text(
                              _primaryDirController.text.isEmpty
                                  ? '选择后将在其中自动创建 everyonenasbackup 子目录存放备份数据'
                                  : _primaryRoot.isEmpty
                                      ? '主目录：${_primaryDirController.text}'
                                      : '写入：$_primaryRoot',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                      AnimatedRotation(
                        turns: _storageExpanded ? 0.5 : 0.0,
                        duration: NeuMotion.resolveState(context),
                        curve: NeuMotion.resolveStateCurve(context),
                        child: const Icon(
                          Icons.keyboard_arrow_down_rounded,
                          color: NeuTokens.textSecondary,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              NeuAnimatedExpand(
                expanded: _storageExpanded,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    const SizedBox(height: 14),
                    const Divider(height: 1),
                    const SizedBox(height: 14),
                    Container(
                      width: double.infinity,
                      padding: const EdgeInsets.symmetric(
                        horizontal: 12,
                        vertical: 10,
                      ),
                      decoration: BoxDecoration(
                        color: NeuTokens.insetSurface,
                        borderRadius:
                            const BorderRadius.all(Radius.circular(10)),
                        border: Border.all(color: NeuTokens.hairlineBorder),
                      ),
                      child: Text(
                        _primaryRoot.isEmpty
                            ? '保存后会自动在所选目录内创建 everyonenasbackup 子目录，所有备份数据只写入该子目录，不会与您自己的文件混放。'
                            : '隔离目录（实际写入位置）：\n$_primaryRoot'
                                '${_fallbackRoot == null ? '' : '\n备用隔离目录：$_fallbackRoot'}',
                        style: textTheme.bodySmall,
                      ),
                    ),
                    const SizedBox(height: 14),
                    if (isWide)
                      Row(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: <Widget>[
                          Expanded(
                            child: NeuInsetField(
                              controller: _primaryDirController,
                              label: primaryDrive == null
                                  ? '主备份文件夹'
                                  : '主备份文件夹（$primaryDrive）',
                              hint: Platform.isAndroid
                                  ? '/storage/emulated/0/EveryoneNAS'
                                  : r'D:\EveryoneNAS_Primary',
                              prefixIcon: Icons.save_outlined,
                            ),
                          ),
                          const SizedBox(width: NeuSpace.sm),
                          NeuButton(
                            label: '浏览',
                            icon: Icons.folder_open_outlined,
                            onPressed: () =>
                                _pickTargetDirectory(isPrimary: true),
                          ),
                        ],
                      )
                    else ...<Widget>[
                      NeuInsetField(
                        controller: _primaryDirController,
                        label: primaryDrive == null
                            ? '主备份文件夹'
                            : '主备份文件夹（$primaryDrive）',
                        hint: Platform.isAndroid
                            ? '/storage/emulated/0/EveryoneNAS'
                            : r'D:\EveryoneNAS_Primary',
                        prefixIcon: Icons.save_outlined,
                      ),
                      const SizedBox(height: NeuSpace.sm),
                      NeuButton(
                        label: '浏览主目录',
                        icon: Icons.folder_open_outlined,
                        expanded: true,
                        onPressed: () => _pickTargetDirectory(isPrimary: true),
                      ),
                    ],
                    const SizedBox(height: 12),
                    if (isWide)
                      Row(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: <Widget>[
                          Expanded(
                            child: NeuInsetField(
                              controller: _fallbackDirController,
                              label: fallbackDrive == null
                                  ? '备用备份文件夹（可选，需不同存储卷）'
                                  : '备用备份文件夹（$fallbackDrive）',
                              hint: Platform.isAndroid
                                  ? '/storage/<SD_UUID>/EveryoneNAS_Backup'
                                  : r'E:\EveryoneNAS_Backup',
                              errorText: sameDriveConflict
                                  ? '备用存储位置不能与主备份位于同一存储卷（当前同为 $primaryDrive）'
                                  : null,
                              prefixIcon: Icons.drive_file_move_outlined,
                            ),
                          ),
                          const SizedBox(width: NeuSpace.sm),
                          NeuButton(
                            label: '浏览',
                            icon: Icons.folder_open_outlined,
                            onPressed: () =>
                                _pickTargetDirectory(isPrimary: false),
                          ),
                        ],
                      )
                    else ...<Widget>[
                      NeuInsetField(
                        controller: _fallbackDirController,
                        label: fallbackDrive == null
                            ? '备用备份文件夹（可选，需不同存储卷）'
                            : '备用备份文件夹（$fallbackDrive）',
                        hint: Platform.isAndroid
                            ? '/storage/<SD_UUID>/EveryoneNAS_Backup'
                            : r'E:\EveryoneNAS_Backup',
                        errorText: sameDriveConflict
                            ? '备用存储位置不能与主备份位于同一存储卷（当前同为 $primaryDrive）'
                            : null,
                        prefixIcon: Icons.drive_file_move_outlined,
                      ),
                      const SizedBox(height: NeuSpace.sm),
                      NeuButton(
                        label: '浏览备用目录',
                        icon: Icons.folder_open_outlined,
                        expanded: true,
                        onPressed: () => _pickTargetDirectory(isPrimary: false),
                      ),
                    ],
                    const SizedBox(height: 14),
                    NeuButton(
                      label: '保存备份目录配置',
                      icon: Icons.check_rounded,
                      tone: NeuButtonTone.primary,
                      loading: _savingTargets,
                      onPressed: _saveBackupTargets,
                    ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _GroupRuleEditorDialog extends StatefulWidget {
  const _GroupRuleEditorDialog({
    required this.bridge,
    required this.deviceGroups,
    required this.mirrorRoots,
    required this.pairedPeers,
    this.editing,
  });

  final LanVaultBridge bridge;
  final List<DeviceGroupViewModel> deviceGroups;
  final List<MirrorRootModel> mirrorRoots;
  final List<PairedPeerModel> pairedPeers;
  final GroupRuleViewModel? editing;

  @override
  State<_GroupRuleEditorDialog> createState() => _GroupRuleEditorDialogState();
}

class _GroupRuleEditorDialogState extends State<_GroupRuleEditorDialog> {
  late String _selectedGroupId;
  late String _mode;
  late final TextEditingController _nameController;
  late final TextEditingController _sourceRootController;
  late final TextEditingController _retentionDaysController;

  final Set<String> _selectedTargets = <String>{};
  final Map<String, String?> _selectedMirrorRootByTarget = <String, String?>{};
  bool _submitting = false;

  @override
  void initState() {
    super.initState();
    final edit = widget.editing;
    _selectedGroupId =
        edit?.rule.groupId ?? widget.deviceGroups.first.group.groupId;
    _mode = edit?.rule.mode ?? 'diff_backup';
    _nameController = TextEditingController(text: edit?.rule.name ?? '');
    _sourceRootController =
        TextEditingController(text: edit?.rule.sourceRoot ?? '');
    _retentionDaysController = TextEditingController(
      text: (edit?.rule.archiveRetentionDays ?? 30).toString(),
    );
    if (edit != null) {
      for (final b in edit.bindings) {
        if (b.enabled && !b.isRevoked) {
          _selectedTargets.add(b.targetDeviceId);
          _selectedMirrorRootByTarget[b.targetDeviceId] = b.mirrorRootId;
        }
      }
    }
  }

  @override
  void dispose() {
    _nameController.dispose();
    _sourceRootController.dispose();
    _retentionDaysController.dispose();
    super.dispose();
  }

  DeviceGroupViewModel get _currentGroup {
    return widget.deviceGroups.firstWhere(
      (g) => g.group.groupId == _selectedGroupId,
      orElse: () => widget.deviceGroups.first,
    );
  }

  String _deviceLabel(String deviceId) {
    for (final p in widget.pairedPeers) {
      if (p.deviceId == deviceId) {
        return p.deviceName.trim().isEmpty ? p.deviceId : p.deviceName.trim();
      }
    }
    return deviceId;
  }

  Future<void> _browseSource() async {
    final picked = await widget.bridge.pickDirectory(
      description: '选择组同步规则的本地源目录',
    );
    if (picked != null && mounted) {
      setState(() {
        _sourceRootController.text = picked;
        if (_nameController.text.trim().isEmpty) {
          _nameController.text = SystemEnvHelper.folderDisplayName(picked);
        }
      });
    }
  }

  Future<void> _submit() async {
    final sourceRoot = _sourceRootController.text.trim();
    if (sourceRoot.isEmpty) {
      NeuToast.warning(context, '请先选择或输入源目录路径');
      return;
    }
    if (_selectedTargets.isEmpty) {
      NeuToast.warning(context, '请至少选择一个目标设备成员');
      return;
    }
    final retentionDays =
        int.tryParse(_retentionDaysController.text.trim()) ?? 30;
    if (retentionDays <= 0) {
      NeuToast.warning(context, '镜像归档保留天数必须大于 0');
      return;
    }

    final isMirror = _mode == 'mirror_sync';
    if (isMirror) {
      for (final targetId in _selectedTargets) {
        final mroot = _selectedMirrorRootByTarget[targetId];
        if (mroot == null || mroot.isEmpty) {
          NeuToast.warning(
            context,
            '镜像模式下必须为目标「${_deviceLabel(targetId)}」选择已登记的镜像目录',
          );
          return;
        }
      }
    }

    setState(() => _submitting = true);
    try {
      final edit = widget.editing;
      final requiresPreview = isMirror &&
          (edit == null ||
              !edit.rule.isMirrorSync ||
              edit.rule.sourceRoot != sourceRoot);

      // 若为镜像模式且需要预览确认，先以停用状态落盘并建立绑定，预览确认后再启用。
      final savedRule = await widget.bridge.upsertGroupRule(
        ruleId: edit?.rule.ruleId,
        groupId: _selectedGroupId,
        name: _nameController.text.trim().isEmpty
            ? SystemEnvHelper.folderDisplayName(sourceRoot)
            : _nameController.text.trim(),
        mode: _mode,
        sourceRoot: sourceRoot,
        archiveRetentionDays: retentionDays,
        enabled: requiresPreview ? false : (edit?.rule.enabled ?? true),
      );

      final existingByTarget = <String, RuleTargetBindingModel>{
        if (edit != null)
          for (final b in edit.bindings) b.targetDeviceId: b,
      };

      for (final targetId in _selectedTargets) {
        final existingBinding = existingByTarget[targetId];
        await widget.bridge.upsertRuleTargetBinding(
          bindingId: existingBinding?.bindingId,
          ruleId: savedRule.ruleId,
          targetDeviceId: targetId,
          targetVaultId: isMirror ? null : 'default',
          mirrorRootId: isMirror ? _selectedMirrorRootByTarget[targetId] : null,
          enabled: true,
        );
      }

      if (edit != null) {
        for (final prev in edit.bindings) {
          if (!_selectedTargets.contains(prev.targetDeviceId) && prev.enabled) {
            await widget.bridge.disableRuleTargetBinding(prev.bindingId);
          }
        }
      }

      if (requiresPreview) {
        final previews = await widget.bridge.previewGroupRule(savedRule.ruleId);
        if (!mounted) return;
        final confirmed = await showNeuDialog<bool>(
          context: context,
          builder: (previewContext) => _MirrorPreviewDialog(
            ruleName: savedRule.name,
            isMirrorMode: true,
            previews: previews,
            pairedPeers: widget.pairedPeers,
            requireConfirmToEnable: true,
          ),
        );
        if (confirmed == true) {
          await widget.bridge.toggleGroupRule(
            ruleId: savedRule.ruleId,
            enabled: true,
          );
          if (!mounted) return;
          NeuToast.success(context, '已确认变更预览并启用镜像规则「${savedRule.name}」');
        } else {
          if (!mounted) return;
          NeuToast.info(
            context,
            '规则「${savedRule.name}」已保存为停用状态，预览确认后可随时启用',
          );
        }
      } else {
        if (!mounted) return;
        NeuToast.success(context, '组规则「${savedRule.name}」已保存');
      }

      Navigator.of(context).pop(true);
    } catch (err) {
      if (!mounted) return;
      setState(() => _submitting = false);
      NeuToast.error(
        context,
        '保存组规则失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final members = _currentGroup.activeMembers;
    final activeRoots =
        widget.mirrorRoots.where((r) => r.isActive).toList(growable: false);

    return NeuDialog(
      title: widget.editing == null ? '新建设备组规则' : '编辑设备组规则',
      subtitle: '配置同步模式、源目录与目标绑定（镜像模式启用前需执行变更预览）',
      maxWidth: 540,
      actions: <Widget>[
        NeuButton(
          label: '取消',
          semanticLabel: '取消编辑组规则',
          onPressed:
              _submitting ? null : () => Navigator.of(context).pop(false),
        ),
        NeuButton(
          label: _mode == 'mirror_sync' ? '保存并预览变更' : '保存规则',
          icon: _mode == 'mirror_sync'
              ? Icons.preview_rounded
              : Icons.check_rounded,
          tone: NeuButtonTone.primary,
          loading: _submitting,
          semanticLabel: '保存设备组规则',
          onPressed: _submit,
        ),
      ],
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          NeuSegmentedControl<String>(
            semanticLabel: '选择同步模式',
            value: _mode,
            onChanged: (val) => setState(() => _mode = val),
            segments: const <NeuSegment<String>>[
              NeuSegment<String>(
                value: 'diff_backup',
                label: '差异备份',
              ),
              NeuSegment<String>(
                value: 'mirror_sync',
                label: '镜像同步',
              ),
            ],
          ),
          const SizedBox(height: 14),
          NeuDropdown<String>(
            label: '所属设备组',
            prefixIcon: Icons.workspaces_outlined,
            value: _selectedGroupId,
            options: widget.deviceGroups
                .map(
                  (g) => NeuDropdownOption<String>(
                    value: g.group.groupId,
                    label: g.group.name,
                    subtitle: '${g.activeMembers.length} 台设备',
                  ),
                )
                .toList(growable: false),
            onChanged: (val) {
              if (val != null) {
                setState(() {
                  _selectedGroupId = val;
                  _selectedTargets.clear();
                });
              }
            },
          ),
          const SizedBox(height: 12),
          NeuInsetField(
            controller: _nameController,
            label: '规则名称',
            hint: '例如：工作文档多机备份',
            prefixIcon: Icons.drive_file_rename_outline_rounded,
          ),
          const SizedBox(height: 12),
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: <Widget>[
              Expanded(
                child: NeuInsetField(
                  controller: _sourceRootController,
                  label: '源目录路径',
                  hint: Platform.isAndroid
                      ? '/storage/emulated/0/Documents'
                      : r'C:\Users\Documents',
                  prefixIcon: Icons.folder_outlined,
                ),
              ),
              const SizedBox(width: NeuSpace.sm),
              NeuButton(
                label: '浏览',
                icon: Icons.folder_open_rounded,
                semanticLabel: '浏览选择源目录',
                onPressed: _browseSource,
              ),
            ],
          ),
          if (_mode == 'mirror_sync') ...<Widget>[
            const SizedBox(height: 12),
            NeuInsetField(
              controller: _retentionDaysController,
              label: '镜像历史归档保留天数（默认 30 天）',
              hint: '30',
              keyboardType: TextInputType.number,
              prefixIcon: Icons.history_toggle_off_rounded,
            ),
            const SizedBox(height: 8),
            Text(
              '镜像同步仅处理常规文件、空目录与可表示 mtime；不支持符号链接与特殊文件。',
              style: textTheme.bodySmall,
            ),
          ],
          const SizedBox(height: 14),
          Text('选择目标成员', style: textTheme.titleSmall),
          const SizedBox(height: 8),
          if (members.isEmpty)
            Text('该设备组暂无活跃成员', style: textTheme.bodySmall)
          else
            Column(
              children: <Widget>[
                for (final member in members) ...<Widget>[
                  Builder(
                    builder: (context) {
                      final isSelected =
                          _selectedTargets.contains(member.deviceId);
                      final deviceRoots = activeRoots
                          .where((r) => r.deviceId == member.deviceId)
                          .toList(growable: false);
                      final selectableRoots =
                          deviceRoots.isNotEmpty ? deviceRoots : activeRoots;
                      return NeuCard(
                        inset: true,
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 8,
                        ),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: <Widget>[
                            NeuSwitch(
                              title: _deviceLabel(member.deviceId),
                              subtitle: member.isCoordinator
                                  ? '本机 / 协调者'
                                  : '成员设备 (${member.deviceId})',
                              semanticLabel:
                                  '选择目标设备 ${_deviceLabel(member.deviceId)}',
                              value: isSelected,
                              onChanged: (checked) {
                                setState(() {
                                  if (checked) {
                                    _selectedTargets.add(member.deviceId);
                                    if (_mode == 'mirror_sync' &&
                                        _selectedMirrorRootByTarget[
                                                member.deviceId] ==
                                            null &&
                                        selectableRoots.isNotEmpty) {
                                      _selectedMirrorRootByTarget[
                                              member.deviceId] =
                                          selectableRoots.first.mirrorRootId;
                                    }
                                  } else {
                                    _selectedTargets.remove(member.deviceId);
                                  }
                                });
                              },
                            ),
                            if (isSelected &&
                                _mode == 'mirror_sync') ...<Widget>[
                              const SizedBox(height: 8),
                              NeuDropdown<String>(
                                label: '目标镜像根目录',
                                emptyHint: '请先点击“登记本机镜像目录”',
                                prefixIcon: Icons.folder_special_outlined,
                                value: _selectedMirrorRootByTarget[
                                    member.deviceId],
                                options: selectableRoots
                                    .map(
                                      (r) => NeuDropdownOption<String>(
                                        value: r.mirrorRootId,
                                        label: r.label,
                                        subtitle: r.canonicalRoot,
                                      ),
                                    )
                                    .toList(growable: false),
                                onChanged: (rootId) {
                                  setState(() {
                                    _selectedMirrorRootByTarget[
                                        member.deviceId] = rootId;
                                  });
                                },
                              ),
                            ],
                          ],
                        ),
                      );
                    },
                  ),
                  const SizedBox(height: 8),
                ],
              ],
            ),
        ],
      ),
    );
  }
}

class _MirrorPreviewDialog extends StatelessWidget {
  const _MirrorPreviewDialog({
    required this.ruleName,
    required this.isMirrorMode,
    required this.previews,
    required this.pairedPeers,
    required this.requireConfirmToEnable,
  });

  final String ruleName;
  final bool isMirrorMode;
  final List<MirrorPreviewModel> previews;
  final List<PairedPeerModel> pairedPeers;
  final bool requireConfirmToEnable;

  String _peerLabel(String deviceId) {
    for (final p in pairedPeers) {
      if (p.deviceId == deviceId) {
        return p.deviceName.trim().isEmpty ? p.deviceId : p.deviceName.trim();
      }
    }
    return deviceId;
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return NeuDialog(
      title: '执行前变更预览 · $ruleName',
      subtitle: isMirrorMode
          ? '镜像同步会在覆盖或移除目标文件前先归档旧版本，请核对以下变更数量与冲突列表。'
          : '核对差异备份本次扫描的新增、更新与变更统计。',
      maxWidth: 520,
      actions: <Widget>[
        NeuButton(
          label: requireConfirmToEnable ? '暂不启用' : '关闭',
          semanticLabel: requireConfirmToEnable ? '暂不启用镜像规则' : '关闭变更预览',
          onPressed: () => Navigator.of(context).pop(false),
        ),
        if (requireConfirmToEnable)
          NeuButton(
            label: '确认预览并启用规则',
            icon: Icons.check_circle_outline_rounded,
            tone: NeuButtonTone.primary,
            semanticLabel: '确认变更预览并启用镜像规则',
            onPressed: () => Navigator.of(context).pop(true),
          ),
      ],
      child: previews.isEmpty
          ? NeuCard(
              inset: true,
              padding: const EdgeInsets.all(14),
              child: Text(
                '当前规则尚未包含可预览的活跃目标绑定。',
                style: textTheme.bodySmall,
              ),
            )
          : Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                for (final report in previews) ...<Widget>[
                  NeuCard(
                    inset: true,
                    padding: const EdgeInsets.all(14),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Text(
                          '目标设备：${_peerLabel(report.targetDeviceId)}',
                          style: textTheme.titleSmall,
                        ),
                        const SizedBox(height: 10),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: <Widget>[
                            NeuStatusBadge(
                              label: '新增 ${report.addCount}',
                              tone: NeuStatusTone.healthy,
                              showDot: false,
                            ),
                            NeuStatusBadge(
                              label: '覆盖 ${report.overwriteCount}',
                              tone: NeuStatusTone.accent,
                              showDot: false,
                            ),
                            NeuStatusBadge(
                              label: '移除 ${report.removeCount}',
                              tone: report.removeCount > 0
                                  ? NeuStatusTone.warning
                                  : NeuStatusTone.neutral,
                              showDot: false,
                            ),
                            NeuStatusBadge(
                              label: '阻塞 ${report.blockedCount}',
                              tone: report.blockedCount > 0
                                  ? NeuStatusTone.danger
                                  : NeuStatusTone.neutral,
                              showDot: false,
                            ),
                            NeuStatusBadge(
                              label: '未变化 ${report.unchangedCount}',
                              tone: NeuStatusTone.neutral,
                              showDot: false,
                            ),
                          ],
                        ),
                        if (report.conflicts.isNotEmpty) ...<Widget>[
                          const SizedBox(height: 10),
                          Text(
                            '冲突与阻塞明细 (${report.conflicts.length})',
                            style: textTheme.labelMedium?.copyWith(
                              color: NeuTokens.danger,
                            ),
                          ),
                          const SizedBox(height: 4),
                          for (final conflict in report.conflicts)
                            Padding(
                              padding: const EdgeInsets.only(top: 3),
                              child: Text(
                                '• $conflict',
                                style: textTheme.bodySmall?.copyWith(
                                  color: NeuTokens.danger,
                                ),
                              ),
                            ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 10),
                ],
              ],
            ),
    );
  }
}

class _CreateBackupTaskDialog extends StatefulWidget {
  const _CreateBackupTaskDialog({
    required this.bridge,
    required this.existingRules,
  });

  final LanVaultBridge bridge;
  final List<SyncRuleModel> existingRules;

  @override
  State<_CreateBackupTaskDialog> createState() =>
      _CreateBackupTaskDialogState();
}

class _CreateBackupTaskDialogState extends State<_CreateBackupTaskDialog> {
  final TextEditingController _dirController = TextEditingController();
  bool _submitting = false;

  @override
  void dispose() {
    _dirController.dispose();
    super.dispose();
  }

  Future<void> _browse() async {
    try {
      final picked = await widget.bridge.pickDirectory(
        description: '选择需要自动备份的本地文件夹',
      );
      if (picked != null && mounted) {
        setState(() => _dirController.text = picked);
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '打开文件夹选择器失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _submit() async {
    final dirPath = _dirController.text.trim();
    if (dirPath.isEmpty) {
      NeuToast.warning(context, '请先选择或输入文件夹路径');
      return;
    }
    final duplicate = widget.existingRules.any(
      (f) => f.localDir.trim().toLowerCase() == dirPath.toLowerCase(),
    );
    if (duplicate) {
      NeuToast.info(context, '该文件夹已在备份任务列表中');
      return;
    }

    setState(() => _submitting = true);
    try {
      final folderName = SystemEnvHelper.folderDisplayName(dirPath)
          .replaceAll(RegExp(r'[^a-zA-Z0-9\u4e00-\u9fa5_-]'), '-');
      final ruleId =
          'folder-${folderName.isEmpty ? 'dir' : folderName}-${DateTime.now().millisecondsSinceEpoch % 100000}';
      await widget.bridge.upsertSyncRule(
        ruleId: ruleId,
        localDir: dirPath,
        targetVaultId: 'default',
        enabled: true,
      );
      if (!mounted) return;
      NeuToast.success(context, '已新建备份任务');
      Navigator.of(context).pop(true);
    } catch (err) {
      if (!mounted) return;
      setState(() => _submitting = false);
      NeuToast.error(
        context,
        '新建任务失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
    final dirHint = Platform.isAndroid
        ? '/storage/emulated/0/DCIM'
        : Platform.environment['USERPROFILE'] != null
            ? '${Platform.environment['USERPROFILE']}\\Documents'
            : r'C:\Users\Documents';

    final field = NeuInsetField(
      controller: _dirController,
      label: '本地源文件夹路径',
      hint: dirHint,
      prefixIcon: Icons.folder_outlined,
      onSubmitted: (_) => _submit(),
    );
    final browseBtn = NeuButton(
      label: '浏览',
      icon: Icons.folder_open_rounded,
      onPressed: _browse,
    );

    return NeuDialog(
      title: '新建备份任务',
      maxWidth: 460,
      actions: <Widget>[
        NeuButton(
          label: '取消',
          onPressed:
              _submitting ? null : () => Navigator.of(context).pop(false),
        ),
        NeuButton(
          label: '添加并启用自动增量备份',
          icon: Icons.check_rounded,
          tone: NeuButtonTone.primary,
          loading: _submitting,
          onPressed: _submit,
        ),
      ],
      child: isWide
          ? Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(child: field),
                const SizedBox(width: NeuSpace.sm),
                browseBtn,
              ],
            )
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                field,
                const SizedBox(height: NeuSpace.sm),
                browseBtn,
              ],
            ),
    );
  }
}

class _TactileCircleFab extends StatefulWidget {
  const _TactileCircleFab({
    required this.onPressed,
    required this.semanticLabel,
  });

  final VoidCallback onPressed;
  final String semanticLabel;

  @override
  State<_TactileCircleFab> createState() => _TactileCircleFabState();
}

class _TactileCircleFabState extends State<_TactileCircleFab> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    final reduced = NeuMotion.isReduced(context);
    final scale = (_pressed && !reduced) ? 0.92 : 1.0;
    return RepaintBoundary(
      child: Semantics(
        button: true,
        label: widget.semanticLabel,
        child: GestureDetector(
          onTapDown: (_) => setState(() => _pressed = true),
          onTapUp: (_) => setState(() => _pressed = false),
          onTapCancel: () => setState(() => _pressed = false),
          onTap: widget.onPressed,
          child: TweenAnimationBuilder<double>(
            tween: Tween<double>(end: scale),
            duration: NeuMotion.resolvePress(context),
            curve: NeuMotion.resolvePressCurve(context),
            builder: (context, s, child) {
              return Transform.scale(scale: s, child: child);
            },
            child: AnimatedContainer(
              duration: NeuMotion.resolvePress(context),
              curve: NeuMotion.resolvePressCurve(context),
              width: 54,
              height: 54,
              decoration: BoxDecoration(
                gradient: NeuTokens.primaryGradient,
                shape: BoxShape.circle,
                boxShadow: _pressed
                    ? NeuTokens.pressedShadows
                    : NeuTokens.primaryButtonShadows,
              ),
              child: const Icon(
                Icons.add_rounded,
                color: Colors.white,
                size: 28,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
