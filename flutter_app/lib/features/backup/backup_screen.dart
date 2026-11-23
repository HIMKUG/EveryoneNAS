import 'dart:async';
import 'dart:io';

import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 3. 备份页：
/// - 右上角 `+ 新建备份任务` 按钮（弹窗支持双端目录选择）
/// - 顶部摘要区：左为实时“备份状态”进度卡，右为“增量备份 (FastCDC)”与“备份策略 (GFS 7/4/12)”卡片
/// - 备份任务列表：暖金文件夹图标 + 实时启停开关（toggle_sync_rule）+ 立即扫描 + 删除
/// - 底部折叠主/备异盘存储目录配置卡（保留同卷冲突校验）
class BackupScreen extends StatefulWidget {
  const BackupScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<BackupScreen> createState() => _BackupScreenState();
}

class _BackupScreenState extends State<BackupScreen> {
  List<SyncRuleModel> _folders = const <SyncRuleModel>[];
  final Set<String> _scanningRuleIds = <String>{};
  final Set<String> _togglingRuleIds = <String>{};
  SyncProgressEvent? _latestSync;
  StreamSubscription<SyncProgressEvent>? _syncSub;

  bool _loading = true;
  bool _savingTargets = false;
  bool _storageExpanded = false;
  bool _syncPaused = false;
  bool _togglingGlobalPause = false;

  final TextEditingController _primaryDirController = TextEditingController();
  final TextEditingController _fallbackDirController = TextEditingController();
  late final Listenable _targetControllersListenable;

  @override
  void initState() {
    super.initState();
    _targetControllersListenable = Listenable.merge(<Listenable>[
      _primaryDirController,
      _fallbackDirController,
    ]);
    _loadData();
    _subscribeSync();
  }

  @override
  void dispose() {
    _syncSub?.cancel();
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
          NeuToast.error(context, '同步进度监听异常：$err');
        },
      );
    } catch (err) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          NeuToast.error(context, '连接同步事件流失败：$err');
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
      ]);
      if (!mounted) return;
      final rules = results[0] as List<SyncRuleModel>;
      final storage = results[1] as BackupStorageConfigModel;
      final paused = results[2] as bool;
      setState(() {
        _folders = rules;
        _primaryDirController.text = storage.primaryDir;
        _fallbackDirController.text = storage.fallbackDir ?? '';
        _syncPaused = paused;
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(context, '加载备份配置失败：$err');
    }
  }

  Future<void> _openCreateTaskDialog() async {
    final created = await showDialog<bool>(
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
      NeuToast.error(context, '切换全局暂停失败：$err');
    }
  }

  Future<void> _changeRuleSchedule(SyncRuleModel folder, String scheduleMode) async {
    try {
      final applied = await widget.bridge.setSyncRuleSchedule(
        ruleId: folder.ruleId,
        scheduleMode: scheduleMode,
      );
      if (!mounted) return;
      setState(() {
        _folders = _folders
            .map((r) =>
                r.ruleId == folder.ruleId ? r.copyWith(scheduleMode: applied) : r)
            .toList(growable: false);
      });
      NeuToast.success(context, '已更新调度策略');
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '更新调度策略失败：$err');
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
            .map((r) =>
                r.ruleId == folder.ruleId ? r.copyWith(enabled: applied) : r)
            .toList(growable: false);
      });
      NeuToast.success(
        context,
        '${SystemEnvHelper.folderDisplayName(folder.localDir)} 已${applied ? '启用' : '暂停'}',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _togglingRuleIds.remove(folder.ruleId));
      NeuToast.error(context, '切换任务状态失败：$err');
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
      NeuToast.error(context, '触发扫描失败：$err');
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
      NeuToast.error(context, '移除失败：$err');
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
      NeuToast.error(context, '打开文件夹选择器失败：$err');
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
      await widget.bridge.setBackupStorageConfig(
        primaryDir: primary,
        fallbackDir: fallback.isEmpty ? null : fallback,
      );
      if (!mounted) return;
      setState(() => _savingTargets = false);
      NeuToast.success(context, '备份存储位置已保存');
    } catch (err) {
      if (!mounted) return;
      setState(() => _savingTargets = false);
      NeuToast.error(context, '保存失败：$err');
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

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= 840;
    final sync = _latestSync;
    final dedupPct = ((sync?.dedupRatio ?? 0.0) * 100).round();

    final list = ListView(
      padding: EdgeInsets.fromLTRB(
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : NeuSpace.md,
        isWide ? NeuSpace.lg : 88,
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
                      '管理备份任务、规则与目录监听',
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
        _buildTaskListCard(context),
        const SizedBox(height: NeuSpace.md),
        _buildStorageConfigCard(context),
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

  Widget _buildBackupStatusCard(BuildContext context, SyncProgressEvent? sync) {
    final textTheme = Theme.of(context).textTheme;
    final processedStr = sync == null
        ? '0 B / 0 B'
        : '${SystemEnvHelper.formatBytes(sync.bytesProcessed)} / ${SystemEnvHelper.formatBytes(sync.totalBytes)}';
    final ratio = sync?.progressRatio ?? 0.0;

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
              if (sync != null && sync.throughputBps > 0) ...<Widget>[
                NeuStatusBadge(
                  label: '${SystemEnvHelper.formatBytes(sync.throughputBps)}/s',
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

  Widget _buildTaskListCard(BuildContext context) {
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
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(12)),
              ),
              child: Text(
                '尚未添加备份目录，点击右上角“+ 新建备份任务”选择需要保护的本地文件夹。',
                style: textTheme.bodySmall,
              ),
            )
          else
            for (var i = 0; i < _folders.length; i++) ...<Widget>[
              Builder(
                builder: (context) {
                  final folder = _folders[i];
                  final isScanning = _scanningRuleIds.contains(folder.ruleId);
                  final isToggling = _togglingRuleIds.contains(folder.ruleId);
                  return Padding(
                    padding: const EdgeInsets.symmetric(vertical: 10),
                    child: Row(
                      children: <Widget>[
                        Container(
                          width: 42,
                          height: 42,
                          decoration: const BoxDecoration(
                            color: NeuTokens.folderGoldSoft,
                            borderRadius: BorderRadius.all(Radius.circular(12)),
                          ),
                          child: const Icon(
                            Icons.folder_rounded,
                            size: 22,
                            color: NeuTokens.folderGold,
                          ),
                        ),
                        const SizedBox(width: 14),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                SystemEnvHelper.folderDisplayName(
                                  folder.localDir,
                                ),
                                style: textTheme.titleSmall?.copyWith(
                                  fontSize: 15,
                                ),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                folder.localDir,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 8),
                        NeuStatusBadge(
                          label: folder.scheduleLabel,
                          tone: folder.scheduleMode == 'manual'
                              ? NeuStatusTone.neutral
                              : NeuStatusTone.accent,
                          showDot: false,
                        ),
                        const SizedBox(width: 8),
                        NeuSwitch(
                          title: '',
                          semanticLabel:
                              '切换 ${SystemEnvHelper.folderDisplayName(folder.localDir)} 自动备份',
                          value: folder.enabled,
                          onChanged: isToggling
                              ? null
                              : (val) => _toggleRule(folder, val),
                        ),
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
                        PopupMenuButton<String>(
                          tooltip: '更多操作',
                          icon: const Icon(
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
                                  Icon(Icons.bolt_rounded, size: 18, color: NeuTokens.accent),
                                  SizedBox(width: 8),
                                  Text('设为：实时 · 增量'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'sched_daily',
                              child: Row(
                                children: <Widget>[
                                  Icon(Icons.today_rounded, size: 18, color: NeuTokens.accent),
                                  SizedBox(width: 8),
                                  Text('设为：每日 · 增量'),
                                ],
                              ),
                            ),
                            PopupMenuItem<String>(
                              value: 'sched_manual',
                              child: Row(
                                children: <Widget>[
                                  Icon(Icons.touch_app_outlined, size: 18, color: NeuTokens.textSecondary),
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

  Widget _buildStorageConfigCard(BuildContext context) {
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
                onTap: () =>
                    setState(() => _storageExpanded = !_storageExpanded),
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
                                  ? '主存储空间满时自动切入备用存储卷（强校验不同盘符）'
                                  : '主目录：${_primaryDirController.text}',
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
                      onPressed: () => _pickTargetDirectory(isPrimary: true),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
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
                      onPressed: () => _pickTargetDirectory(isPrimary: false),
                    ),
                  ],
                ),
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
      NeuToast.error(context, '打开文件夹选择器失败：$err');
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
      NeuToast.error(context, '新建任务失败：$err');
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final dirHint = Platform.isAndroid
        ? '/storage/emulated/0/DCIM'
        : r'C:\Users\YourName\Documents';

    return Dialog(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460),
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: <Widget>[
                  Text('新建备份任务', style: textTheme.titleMedium?.copyWith(fontSize: 18)),
                  IconButton(
                    icon: const Icon(Icons.close_rounded, size: 20),
                    onPressed: () => Navigator.of(context).pop(false),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: <Widget>[
                  Expanded(
                    child: NeuInsetField(
                      controller: _dirController,
                      label: '本地源文件夹路径',
                      hint: dirHint,
                      prefixIcon: Icons.folder_outlined,
                      onSubmitted: (_) => _submit(),
                    ),
                  ),
                  const SizedBox(width: NeuSpace.sm),
                  NeuButton(
                    label: '浏览',
                    icon: Icons.folder_open_rounded,
                    onPressed: _browse,
                  ),
                ],
              ),
              const SizedBox(height: 18),
              NeuButton(
                label: '添加并启用自动增量备份',
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
