import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

enum _SnapshotTab { timeline, catalogArchive, scrub, restoreHistory }

class _RestoreRecord {
  const _RestoreRecord({
    required this.snapshotId,
    required this.vaultId,
    required this.fileCount,
    required this.totalBytes,
    required this.outputPath,
    required this.triggeredAt,
  });

  final String snapshotId;
  final String vaultId;
  final int fileCount;
  final int totalBytes;
  final String outputPath;
  final DateTime triggeredAt;
}

class _BindingOptionItem {
  const _BindingOptionItem({
    required this.rule,
    required this.binding,
  });

  final GroupSyncRuleModel rule;
  final RuleTargetBindingModel binding;

  String get label =>
      '${rule.name} (${rule.modeLabel}) -> ${binding.targetDeviceId}';
}

/// 5. 快照页：
/// - 顶部 Segmented 标签切换：版本时间线 / 版本归档 / 修复校验 / 恢复记录
/// - 桌面端横向时间线节点轴 + 移动端纵向时间线节点轴，绑定 list_snapshots 真实数据
/// - 版本归档（v2 Catalog Archive）：按组规则目标绑定浏览游标分页目录、筛选源已删除条目、查看单文件版本历史并恢复到安全目录
/// - 修复校验页内置一键触发 trigger_bit_rot_scrub 巡检并展示已校验/损坏/自愈块数
class SnapshotsScreen extends StatefulWidget {
  const SnapshotsScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<SnapshotsScreen> createState() => _SnapshotsScreenState();
}

class _SnapshotsScreenState extends State<SnapshotsScreen> {
  static const String _allVaultsSentinel = '__all_vaults__';
  static const int _catalogPageSize = 50;

  _SnapshotTab _activeTab = _SnapshotTab.timeline;
  List<VaultSummaryModel> _vaults = const <VaultSummaryModel>[];
  List<SnapshotSummaryModel> _snapshots = const <SnapshotSummaryModel>[];
  final List<_RestoreRecord> _restoreHistory = <_RestoreRecord>[];

  // v2 Catalog Archive state
  List<_BindingOptionItem> _bindingOptions = const <_BindingOptionItem>[];
  String? _selectedBindingId;
  bool _sourceDeletedOnly = false;
  List<CatalogEntryModel> _catalogEntries = const <CatalogEntryModel>[];
  bool _loadingCatalog = false;
  bool _loadingMoreCatalog = false;
  bool _hasMoreCatalog = false;

  String _selectedVaultId = _allVaultsSentinel;
  String? _selectedSnapshotId;
  BitRotScrubResultModel? _lastScrubResult;

  bool _loading = true;
  bool _scrubbing = false;
  bool _restoring = false;

  @override
  void initState() {
    super.initState();
    _loadData();
  }

  Future<void> _loadData() async {
    setState(() => _loading = true);
    try {
      final vaultFilter =
          _selectedVaultId == _allVaultsSentinel ? null : _selectedVaultId;
      final results = await Future.wait<Object>(<Future<Object>>[
        widget.bridge.listVaults(),
        widget.bridge.listSnapshots(vaultId: vaultFilter),
        widget.bridge.listGroupRules(),
      ]);
      if (!mounted) return;
      final vaults = results[0] as List<VaultSummaryModel>;
      final snapshots = results[1] as List<SnapshotSummaryModel>;
      final groupRules = results[2] as List<GroupRuleViewModel>;
      final bindings = <_BindingOptionItem>[
        for (final view in groupRules)
          for (final b in view.bindings)
            _BindingOptionItem(rule: view.rule, binding: b),
      ];

      String? nextBindingId = _selectedBindingId;
      if (bindings.isEmpty) {
        nextBindingId = null;
      } else if (nextBindingId == null ||
          !bindings.any((item) => item.binding.bindingId == nextBindingId)) {
        nextBindingId = bindings.first.binding.bindingId;
      }

      setState(() {
        _vaults = vaults;
        _snapshots = snapshots;
        _bindingOptions = bindings;
        _selectedBindingId = nextBindingId;
        if (snapshots.isNotEmpty &&
            (_selectedSnapshotId == null ||
                !snapshots.any((s) => s.snapshotId == _selectedSnapshotId))) {
          _selectedSnapshotId = snapshots.first.snapshotId;
        } else if (snapshots.isEmpty) {
          _selectedSnapshotId = null;
        }
        _loading = false;
      });

      if (nextBindingId != null) {
        await _loadCatalogEntries(reset: true);
      } else if (mounted) {
        setState(() {
          _catalogEntries = const <CatalogEntryModel>[];
          _hasMoreCatalog = false;
        });
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      NeuToast.error(
        context,
        '加载快照列表失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  int _catalogRequest = 0;

  Future<void> _loadCatalogEntries({required bool reset}) async {
    final bindingId = _selectedBindingId;
    if (bindingId == null) return;
    if (reset) {
      setState(() {
        _loadingCatalog = true;
        _loadingMoreCatalog = false;
        _catalogEntries = const [];
      });
    } else {
      if (_loadingCatalog || _loadingMoreCatalog || !_hasMoreCatalog) return;
      setState(() => _loadingMoreCatalog = true);
    }

    final request = ++_catalogRequest;
    try {
      final cursor = reset || _catalogEntries.isEmpty
          ? null
          : _catalogEntries.last.pathKey;
      final page = await widget.bridge.listBindingCatalog(
        bindingId: bindingId,
        afterPathKey: cursor,
        limit: _catalogPageSize,
        sourceDeletedOnly: _sourceDeletedOnly,
      );
      if (!mounted ||
          request != _catalogRequest ||
          bindingId != _selectedBindingId) {
        return;
      }
      setState(() {
        _catalogEntries =
            reset ? page : <CatalogEntryModel>[..._catalogEntries, ...page];
        _hasMoreCatalog = page.length >= _catalogPageSize;
        _loadingCatalog = false;
        _loadingMoreCatalog = false;
      });
    } catch (err) {
      if (!mounted ||
          request != _catalogRequest ||
          bindingId != _selectedBindingId) {
        return;
      }
      setState(() {
        _loadingCatalog = false;
        _loadingMoreCatalog = false;
      });
      NeuToast.error(
        context,
        '加载归档目录失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _openCatalogVersionsDialog(CatalogEntryModel entry) async {
    final restored = await showNeuDialog<RestoreCatalogVersionResultModel>(
      context: context,
      builder: (dialogContext) => _CatalogVersionHistoryDialog(
        bridge: widget.bridge,
        entry: entry,
      ),
    );
    if (restored != null && mounted) {
      setState(() {
        _restoreHistory.insert(
          0,
          _RestoreRecord(
            snapshotId: restored.versionId,
            vaultId: restored.bindingId,
            fileCount: 1,
            totalBytes: restored.restoredBytes,
            outputPath: restored.outputPath,
            triggeredAt: DateTime.now(),
          ),
        );
      });
    }
  }

  Future<void> _runBitRotScrub() async {
    setState(() => _scrubbing = true);
    try {
      final result = await widget.bridge.triggerBitRotScrub();
      if (!mounted) return;
      setState(() {
        _lastScrubResult = result;
        _scrubbing = false;
      });
      if (result.corruptedChunks == 0) {
        NeuToast.success(
          context,
          '校验完成：已检查 ${result.checkedChunks} 个数据块，未发现损坏',
        );
      } else {
        NeuToast.warning(
          context,
          '校验发现 ${result.corruptedChunks} 个损坏块，已自动修复 ${result.healedChunks} 个',
        );
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _scrubbing = false);
      NeuToast.error(
        context,
        '执行完整性校验失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  void _openSnapshotDetail(SnapshotSummaryModel snapshot) {
    showNeuDialog<void>(
      context: context,
      builder: (dialogContext) {
        return NeuDialog(
          title: '快照详情',
          maxWidth: 440,
          actions: <Widget>[
            NeuButton(
              label: '关闭',
              onPressed: () => Navigator.of(dialogContext).pop(),
            ),
            NeuButton(
              label: '恢复此快照',
              icon: Icons.restore_rounded,
              tone: NeuButtonTone.primary,
              onPressed: () {
                Navigator.of(dialogContext).pop();
                _triggerRestore(snapshot);
              },
            ),
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              NeuKeyValueRow(label: '快照编号', value: snapshot.snapshotId),
              NeuKeyValueRow(label: '所属存储池', value: snapshot.vaultId),
              NeuKeyValueRow(label: '来源设备', value: snapshot.deviceId),
              NeuKeyValueRow(label: '保留层级', value: snapshot.retentionLabel),
              NeuKeyValueRow(label: '文件数量', value: '${snapshot.fileCount} 个文件'),
              NeuKeyValueRow(
                label: '总容量',
                value: SystemEnvHelper.formatBytes(snapshot.totalBytes),
              ),
              NeuKeyValueRow(label: '创建时间', value: snapshot.createdAt),
            ],
          ),
        );
      },
    );
  }

  Future<void> _triggerRestore(SnapshotSummaryModel snapshot) async {
    if (_restoring) return;
    try {
      final targetDir = await widget.bridge.pickDirectory(
        description: '选择快照恢复目标文件夹',
      );
      if (targetDir == null || !mounted) return;
      setState(() => _restoring = true);
      NeuToast.info(context, '正在解密并校验恢复快照 ${snapshot.snapshotId}…');
      final res = await widget.bridge.restoreSnapshot(
        snapshotId: snapshot.snapshotId,
        targetDir: targetDir,
      );
      if (!mounted) return;
      setState(() {
        _restoring = false;
        _restoreHistory.insert(
          0,
          _RestoreRecord(
            snapshotId: snapshot.snapshotId,
            vaultId: snapshot.vaultId,
            fileCount: res.restoredChunks,
            totalBytes: res.restoredBytes,
            outputPath: res.outputPath,
            triggeredAt: DateTime.now(),
          ),
        );
      });
      NeuToast.success(
        context,
        '快照已恢复（${res.restoredChunks} 块，${SystemEnvHelper.formatBytes(res.restoredBytes)}）-> ${res.outputPath}',
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _restoring = false);
      NeuToast.error(
        context,
        '恢复快照失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  NeuStatusTone _tierTone(String tier) {
    return switch (tier.toLowerCase()) {
      'daily' => NeuStatusTone.accent,
      'weekly' => NeuStatusTone.healthy,
      'monthly' => NeuStatusTone.warning,
      _ => NeuStatusTone.neutral,
    };
  }

  String _formatUnixSecs(int secs) {
    if (secs <= 0) return '-';
    final dt = DateTime.fromMillisecondsSinceEpoch(secs * 1000);
    return '${dt.year}-${dt.month.toString().padLeft(2, '0')}-${dt.day.toString().padLeft(2, '0')} '
        '${dt.hour.toString().padLeft(2, '0')}:${dt.minute.toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    final vaultOptions = <NeuDropdownOption<String>>[
      const NeuDropdownOption<String>(
        value: _allVaultsSentinel,
        label: '全部存储池',
      ),
      for (final v in _vaults)
        NeuDropdownOption<String>(
          value: v.vaultId,
          label: v.name,
        ),
    ];

    return SingleChildScrollView(
      padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (isWide) ...<Widget>[
            Row(
              children: <Widget>[
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text('快照', style: textTheme.headlineSmall),
                      const SizedBox(height: 4),
                      Text(
                        '基于 GFS 的历史版本、组归档目录与数据恢复',
                        style: textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                SizedBox(
                  width: 210,
                  child: NeuDropdown<String>(
                    options: vaultOptions,
                    value: _selectedVaultId,
                    prefixIcon: Icons.layers_outlined,
                    onChanged: (val) {
                      if (val != null) {
                        setState(() => _selectedVaultId = val);
                        _loadData();
                      }
                    },
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '刷新',
                  icon: Icons.refresh_rounded,
                  loading: _loading,
                  onPressed: _loadData,
                ),
              ],
            ),
            const SizedBox(height: 18),
          ] else ...<Widget>[
            Row(
              children: <Widget>[
                Expanded(
                  child: NeuDropdown<String>(
                    options: vaultOptions,
                    value: _selectedVaultId,
                    prefixIcon: Icons.layers_outlined,
                    onChanged: (val) {
                      if (val != null) {
                        setState(() => _selectedVaultId = val);
                        _loadData();
                      }
                    },
                  ),
                ),
                const SizedBox(width: NeuSpace.sm),
                NeuButton(
                  label: '刷新',
                  icon: Icons.refresh_rounded,
                  loading: _loading,
                  onPressed: _loadData,
                ),
              ],
            ),
            const SizedBox(height: 14),
          ],
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560),
            child: NeuSegmentedControl<_SnapshotTab>(
              semanticLabel: '快照功能分类',
              value: _activeTab,
              onChanged: (tab) => setState(() => _activeTab = tab),
              segments: const <NeuSegment<_SnapshotTab>>[
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.timeline,
                  label: '版本时间线',
                ),
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.catalogArchive,
                  label: '版本归档',
                ),
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.scrub,
                  label: '修复校验',
                ),
                NeuSegment<_SnapshotTab>(
                  value: _SnapshotTab.restoreHistory,
                  label: '恢复记录',
                ),
              ],
            ),
          ),
          const SizedBox(height: NeuSpace.md),
          NeuMotionSwitcher(
            child: KeyedSubtree(
              key: ValueKey<_SnapshotTab>(_activeTab),
              child: switch (_activeTab) {
                _SnapshotTab.timeline => _buildTimelineView(context, isWide),
                _SnapshotTab.catalogArchive =>
                  _buildCatalogArchiveView(context, isWide),
                _SnapshotTab.scrub => _buildScrubView(context),
                _SnapshotTab.restoreHistory =>
                  _buildRestoreHistoryView(context),
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCatalogArchiveView(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;

    if (_bindingOptions.isEmpty) {
      return const NeuEmptyState(
        icon: Icons.archive_outlined,
        title: '暂无设备组规则绑定',
        subtitle: '在“备份”页创建设备组同步规则并完成首次同步后，可在此按文件浏览与恢复历史归档版本。',
      );
    }

    return NeuCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (isWide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: <Widget>[
                Expanded(
                  child: NeuDropdown<String>(
                    label: '选择规则目标绑定',
                    prefixIcon: Icons.hub_outlined,
                    value: _selectedBindingId,
                    options: _bindingOptions
                        .map(
                          (item) => NeuDropdownOption<String>(
                            value: item.binding.bindingId,
                            label: item.label,
                            subtitle: item.rule.sourceRoot,
                          ),
                        )
                        .toList(growable: false),
                    onChanged: (val) {
                      if (val != null) {
                        setState(() => _selectedBindingId = val);
                        _loadCatalogEntries(reset: true);
                      }
                    },
                  ),
                ),
                const SizedBox(width: NeuSpace.md),
                SizedBox(
                  width: 200,
                  child: NeuSwitch(
                    title: '仅看源已删除',
                    semanticLabel: '仅查看源端已删除的归档文件',
                    value: _sourceDeletedOnly,
                    onChanged: (val) {
                      setState(() => _sourceDeletedOnly = val);
                      _loadCatalogEntries(reset: true);
                    },
                  ),
                ),
              ],
            )
          else ...<Widget>[
            NeuDropdown<String>(
              label: '选择规则目标绑定',
              prefixIcon: Icons.hub_outlined,
              value: _selectedBindingId,
              options: _bindingOptions
                  .map(
                    (item) => NeuDropdownOption<String>(
                      value: item.binding.bindingId,
                      label: item.label,
                      subtitle: item.rule.sourceRoot,
                    ),
                  )
                  .toList(growable: false),
              onChanged: (val) {
                if (val != null) {
                  setState(() => _selectedBindingId = val);
                  _loadCatalogEntries(reset: true);
                }
              },
            ),
            const SizedBox(height: 10),
            NeuSwitch(
              title: '仅看源已删除',
              semanticLabel: '仅查看源端已删除的归档文件',
              value: _sourceDeletedOnly,
              onChanged: (val) {
                setState(() => _sourceDeletedOnly = val);
                _loadCatalogEntries(reset: true);
              },
            ),
          ],
          const SizedBox(height: 12),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
            decoration: BoxDecoration(
              color: NeuTokens.insetSurface,
              borderRadius: const BorderRadius.all(Radius.circular(10)),
              border: Border.all(color: NeuTokens.hairlineBorder),
            ),
            child: Text(
              '安全恢复提示：镜像与归档历史版本默认恢复到您指定的安全目录，避免写回源目录或在下次镜像同步时被覆盖。',
              style: textTheme.bodySmall,
            ),
          ),
          const SizedBox(height: 14),
          if (_loadingCatalog && _catalogEntries.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 16),
              child: Center(
                child: Text('正在加载目录归档…', style: textTheme.bodySmall),
              ),
            )
          else if (_catalogEntries.isEmpty)
            NeuEmptyState(
              icon: Icons.inventory_2_outlined,
              title: _sourceDeletedOnly ? '暂无源端已删除的文件' : '该绑定暂无归档条目',
              subtitle: _sourceDeletedOnly
                  ? '关闭“仅看源已删除”可查看全部已同步路径。'
                  : '执行一次组规则同步后，此处将展示可追溯版本的目录清单。',
            )
          else ...<Widget>[
            for (var i = 0; i < _catalogEntries.length; i++) ...<Widget>[
              Builder(
                key: ValueKey<String>(
                  'cat_${_catalogEntries[i].bindingId}_${_catalogEntries[i].pathKey}',
                ),
                builder: (context) {
                  final entry = _catalogEntries[i];
                  return Padding(
                    padding: const EdgeInsets.symmetric(vertical: 10),
                    child: Row(
                      children: <Widget>[
                        NeuIconBadge(
                          icon: entry.isDir
                              ? Icons.folder_rounded
                              : Icons.insert_drive_file_rounded,
                          color: entry.sourceDeleted
                              ? NeuTokens.warning
                              : (entry.isDir
                                  ? NeuTokens.folderGold
                                  : NeuTokens.accent),
                          backgroundColor: entry.sourceDeleted
                              ? NeuTokens.folderGoldSoft
                              : (entry.isDir
                                  ? NeuTokens.folderGoldSoft
                                  : NeuTokens.accentSoft),
                          radius: 10,
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
                                  Text(
                                    entry.displayPath,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: textTheme.titleSmall,
                                  ),
                                  if (entry.sourceDeleted)
                                    const NeuStatusBadge(
                                      label: '源已删除',
                                      tone: NeuStatusTone.warning,
                                      showDot: false,
                                    ),
                                ],
                              ),
                              const SizedBox(height: 2),
                              Text(
                                '${SystemEnvHelper.formatBytes(entry.size)} · 更新于 ${_formatUnixSecs(entry.updatedAt)} · Rev ${entry.updatedRevision}',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 8),
                        NeuButton(
                          label: '历史版本',
                          icon: Icons.history_rounded,
                          semanticLabel: '查看 ${entry.displayPath} 的历史版本',
                          onPressed: () => _openCatalogVersionsDialog(entry),
                        ),
                      ],
                    ),
                  );
                },
              ),
              if (i < _catalogEntries.length - 1) const Divider(height: 1),
            ],
            if (_hasMoreCatalog) ...<Widget>[
              const SizedBox(height: 14),
              Center(
                child: NeuButton(
                  label: '加载更多条目',
                  icon: Icons.expand_more_rounded,
                  loading: _loadingMoreCatalog,
                  semanticLabel: '加载下一页目录归档条目',
                  onPressed: () => _loadCatalogEntries(reset: false),
                ),
              ),
            ],
          ],
        ],
      ),
    );
  }

  Widget _buildTimelineView(BuildContext context, bool isWide) {
    final textTheme = Theme.of(context).textTheme;

    if (_loading && _snapshots.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(24),
        child: Center(
          child: Text('正在加载历史快照…', style: textTheme.bodySmall),
        ),
      );
    }

    if (_snapshots.isEmpty) {
      return const NeuEmptyState(
        icon: Icons.history_rounded,
        title: '暂无历史快照',
        subtitle: '完成首次增量备份后，系统将按 GFS（7日 / 4周 / 12月）策略自动生成历史版本时间线。',
      );
    }

    final selectedSnap = _snapshots.firstWhere(
      (s) => s.snapshotId == _selectedSnapshotId,
      orElse: () => _snapshots.first,
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: isWide
              ? _buildHorizontalTimeline(context, selectedSnap)
              : _buildVerticalTimeline(context, selectedSnap),
        ),
        const SizedBox(height: NeuSpace.md),
        NeuCard(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Row(
                children: <Widget>[
                  Expanded(
                    child: Text(
                      '可恢复的快照 (${_snapshots.length})',
                      style: textTheme.titleMedium,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Flexible(
                    child: Text(
                      '已选：${selectedSnap.createdAt}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: textTheme.labelMedium,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 14),
              for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
                Builder(
                  key: ValueKey<String>(_snapshots[i].snapshotId),
                  builder: (context) {
                    final snap = _snapshots[i];
                    final isSelected =
                        snap.snapshotId == selectedSnap.snapshotId;
                    final infoMeta = Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Row(
                          children: <Widget>[
                            Flexible(
                              child: Text(
                                snap.snapshotId,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.titleSmall,
                              ),
                            ),
                            const SizedBox(width: 8),
                            NeuStatusBadge(
                              label: snap.retentionLabel,
                              tone: _tierTone(snap.retentionTier),
                              showDot: false,
                            ),
                          ],
                        ),
                        const SizedBox(height: 3),
                        Text(
                          '${snap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(snap.totalBytes)} · ${snap.createdAt}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: textTheme.bodySmall,
                        ),
                      ],
                    );

                    return InkWell(
                      onTap: () =>
                          setState(() => _selectedSnapshotId = snap.snapshotId),
                      borderRadius: const BorderRadius.all(Radius.circular(12)),
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 10,
                        ),
                        decoration: BoxDecoration(
                          color: isSelected
                              ? NeuTokens.accentSoft
                              : Colors.transparent,
                          borderRadius:
                              const BorderRadius.all(Radius.circular(12)),
                        ),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: <Widget>[
                            Row(
                              children: <Widget>[
                                const NeuIconBadge(
                                  icon: Icons.folder_rounded,
                                  color: NeuTokens.folderGold,
                                  backgroundColor: NeuTokens.folderGoldSoft,
                                  radius: 10,
                                ),
                                const SizedBox(width: 12),
                                Expanded(child: infoMeta),
                                if (isWide) ...<Widget>[
                                  const SizedBox(width: 8),
                                  NeuButton(
                                    label: '浏览',
                                    onPressed: () => _openSnapshotDetail(snap),
                                  ),
                                  const SizedBox(width: 8),
                                  NeuButton(
                                    label: '恢复',
                                    tone: NeuButtonTone.primary,
                                    onPressed: () => _triggerRestore(snap),
                                  ),
                                ],
                              ],
                            ),
                            if (!isWide) ...<Widget>[
                              const SizedBox(height: 10),
                              Row(
                                mainAxisAlignment: MainAxisAlignment.end,
                                children: <Widget>[
                                  NeuButton(
                                    label: '浏览详情',
                                    onPressed: () => _openSnapshotDetail(snap),
                                  ),
                                  const SizedBox(width: 8),
                                  NeuButton(
                                    label: '恢复快照',
                                    tone: NeuButtonTone.primary,
                                    onPressed: () => _triggerRestore(snap),
                                  ),
                                ],
                              ),
                            ],
                          ],
                        ),
                      ),
                    );
                  },
                ),
                if (i < _snapshots.length - 1) const Divider(height: 1),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildHorizontalTimeline(
    BuildContext context,
    SnapshotSummaryModel selectedSnap,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: <Widget>[
        Text(
          selectedSnap.createdAt,
          style: textTheme.titleSmall?.copyWith(color: NeuTokens.accent),
        ),
        const SizedBox(height: 4),
        Text(
          '${selectedSnap.retentionLabel} · ${selectedSnap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(selectedSnap.totalBytes)}',
          style: textTheme.bodySmall,
        ),
        const SizedBox(height: 18),
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Row(
            children: <Widget>[
              for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
                Builder(
                  builder: (context) {
                    final snap = _snapshots[i];
                    final isSelected =
                        snap.snapshotId == selectedSnap.snapshotId;
                    return Semantics(
                      key: ValueKey<String>('h_node_${snap.snapshotId}'),
                      button: true,
                      selected: isSelected,
                      label: '${snap.createdAt} ${snap.retentionLabel} 快照',
                      child: InkWell(
                        borderRadius:
                            const BorderRadius.all(Radius.circular(10)),
                        onTap: () => setState(
                          () => _selectedSnapshotId = snap.snapshotId,
                        ),
                        child: Column(
                          children: <Widget>[
                            AnimatedContainer(
                              duration: NeuMotion.resolveState(context),
                              curve: NeuMotion.resolveStateCurve(context),
                              width: isSelected ? 34 : 16,
                              height: isSelected ? 34 : 16,
                              decoration: BoxDecoration(
                                color: isSelected
                                    ? NeuTokens.accent
                                    : NeuTokens.timelineInactive,
                                shape: BoxShape.circle,
                                border: isSelected
                                    ? Border.all(
                                        color: NeuTokens.timelineHalo,
                                        width: 5,
                                      )
                                    : null,
                                boxShadow: isSelected
                                    ? NeuTokens.primaryButtonShadows
                                    : null,
                              ),
                              child: isSelected
                                  ? const Center(
                                      child: SizedBox(
                                        width: 8,
                                        height: 8,
                                        child: DecoratedBox(
                                          decoration: BoxDecoration(
                                            color: Colors.white,
                                            shape: BoxShape.circle,
                                          ),
                                        ),
                                      ),
                                    )
                                  : null,
                            ),
                            const SizedBox(height: 8),
                            SizedBox(
                              width: 92,
                              child: Text(
                                snap.createdAt,
                                textAlign: TextAlign.center,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.labelMedium?.copyWith(
                                  fontSize: 11,
                                  color: isSelected
                                      ? NeuTokens.accent
                                      : NeuTokens.textSecondary,
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    );
                  },
                ),
                if (i < _snapshots.length - 1)
                  Container(
                    width: 44,
                    height: 2,
                    margin: const EdgeInsets.only(bottom: 22),
                    color: NeuTokens.timelineLine,
                  ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildVerticalTimeline(
    BuildContext context,
    SnapshotSummaryModel selectedSnap,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        for (var i = 0; i < _snapshots.length; i++) ...<Widget>[
          Builder(
            builder: (context) {
              final snap = _snapshots[i];
              final isSelected = snap.snapshotId == selectedSnap.snapshotId;
              return Semantics(
                key: ValueKey<String>('v_node_${snap.snapshotId}'),
                button: true,
                selected: isSelected,
                label: '${snap.createdAt} ${snap.retentionLabel} 快照',
                child: InkWell(
                  borderRadius: const BorderRadius.all(Radius.circular(10)),
                  onTap: () =>
                      setState(() => _selectedSnapshotId = snap.snapshotId),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Column(
                        children: <Widget>[
                          Container(
                            width: isSelected ? 22 : 12,
                            height: isSelected ? 22 : 12,
                            margin: EdgeInsets.symmetric(
                              horizontal: isSelected ? 0 : 5,
                              vertical: 2,
                            ),
                            decoration: BoxDecoration(
                              color: isSelected
                                  ? NeuTokens.accent
                                  : NeuTokens.timelineInactive,
                              shape: BoxShape.circle,
                              border: isSelected
                                  ? Border.all(
                                      color: NeuTokens.timelineHalo,
                                      width: 4,
                                    )
                                  : null,
                            ),
                          ),
                          if (i < _snapshots.length - 1)
                            Container(
                              width: 2,
                              height: 36,
                              color: NeuTokens.timelineLine,
                            ),
                        ],
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Padding(
                          padding: const EdgeInsets.only(bottom: 12),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                snap.createdAt,
                                style: textTheme.titleSmall?.copyWith(
                                  color: isSelected
                                      ? NeuTokens.accent
                                      : NeuTokens.textPrimary,
                                ),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                '${snap.retentionLabel} · ${snap.fileCount} 个文件 · ${SystemEnvHelper.formatBytes(snap.totalBytes)}',
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              );
            },
          ),
        ],
        const SizedBox(height: 8),
        NeuButton(
          label: '预览并恢复所选版本',
          icon: Icons.restore_rounded,
          tone: NeuButtonTone.primary,
          expanded: true,
          onPressed: () => _openSnapshotDetail(selectedSnap),
        ),
      ],
    );
  }

  Widget _buildScrubView(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final scrub = _lastScrubResult;

    return NeuCard(
      padding: const EdgeInsets.all(22),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Row(
            children: <Widget>[
              Container(
                width: 44,
                height: 44,
                decoration: const BoxDecoration(
                  color: NeuTokens.healthySoft,
                  borderRadius: BorderRadius.all(Radius.circular(12)),
                ),
                child: const Icon(
                  Icons.health_and_safety_rounded,
                  size: 24,
                  color: NeuTokens.healthy,
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text('Bit-Rot 数据完整性校验与自愈', style: textTheme.titleMedium),
                    const SizedBox(height: 2),
                    Text(
                      '逐块重算 BLAKE3 校验和，自动识别静默位翻转并利用副本自愈修复。',
                      style: textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 18),
          NeuButton(
            label: _scrubbing ? '正在逐块校验…' : '立即触发 Bit-Rot 巡检',
            icon: Icons.fact_check_outlined,
            tone: NeuButtonTone.primary,
            loading: _scrubbing,
            onPressed: _runBitRotScrub,
          ),
          const SizedBox(height: 18),
          if (scrub == null)
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: NeuTokens.insetSurface,
                borderRadius: const BorderRadius.all(Radius.circular(12)),
              ),
              child: Text(
                '尚未在当前会话触发巡检。点击上方按钮立即对本地存储池数据块执行完整性校验。',
                style: textTheme.bodySmall,
              ),
            )
          else
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Row(
                  children: <Widget>[
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '已校验块数',
                        value: '${scrub.checkedChunks}',
                        color: NeuTokens.accent,
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '损坏块数',
                        value: '${scrub.corruptedChunks}',
                        color: scrub.corruptedChunks > 0
                            ? NeuTokens.danger
                            : NeuTokens.healthy,
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: _ScrubMetricTile(
                        label: '已自愈块数',
                        value: '${scrub.healedChunks}',
                        color: NeuTokens.healthy,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                Text(
                  '巡检任务：${scrub.scrubId} · 状态：${scrub.status}',
                  style: textTheme.bodySmall,
                ),
              ],
            ),
        ],
      ),
    );
  }

  Widget _buildRestoreHistoryView(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    if (_restoreHistory.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(28),
        child: Center(
          child: Text(
            '暂无恢复记录。在“版本时间线”或“版本归档”中恢复数据后将在此展示。',
            textAlign: TextAlign.center,
            style: textTheme.bodySmall,
          ),
        ),
      );
    }

    return NeuCard(
      padding: const EdgeInsets.all(18),
      child: Column(
        children: <Widget>[
          for (var i = 0; i < _restoreHistory.length; i++) ...<Widget>[
            Builder(
              builder: (context) {
                final rec = _restoreHistory[i];
                final ts =
                    '${rec.triggeredAt.hour.toString().padLeft(2, '0')}:${rec.triggeredAt.minute.toString().padLeft(2, '0')}:${rec.triggeredAt.second.toString().padLeft(2, '0')}';
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  child: Row(
                    children: <Widget>[
                      const Icon(
                        Icons.restore_rounded,
                        color: NeuTokens.accent,
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(rec.snapshotId, style: textTheme.titleSmall),
                            Text(
                              '${rec.outputPath} · ${SystemEnvHelper.formatBytes(rec.totalBytes)}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 8),
                      Text(ts, style: textTheme.labelMedium),
                    ],
                  ),
                );
              },
            ),
            if (i < _restoreHistory.length - 1) const Divider(height: 1),
          ],
        ],
      ),
    );
  }
}

class _CatalogVersionHistoryDialog extends StatefulWidget {
  const _CatalogVersionHistoryDialog({
    required this.bridge,
    required this.entry,
  });

  final LanVaultBridge bridge;
  final CatalogEntryModel entry;

  @override
  State<_CatalogVersionHistoryDialog> createState() =>
      _CatalogVersionHistoryDialogState();
}

class _CatalogVersionHistoryDialogState
    extends State<_CatalogVersionHistoryDialog> {
  static const int _pageSize = 30;

  List<CatalogVersionModel> _versions = const <CatalogVersionModel>[];
  bool _loading = true;
  bool _loadingMore = false;
  bool _hasMore = false;
  String? _restoringVersionId;

  @override
  void initState() {
    super.initState();
    _loadVersions(reset: true);
  }

  Future<void> _loadVersions({required bool reset}) async {
    if (reset) {
      setState(() => _loading = true);
    } else {
      if (_loadingMore || !_hasMore) return;
      setState(() => _loadingMore = true);
    }

    try {
      final last = reset || _versions.isEmpty ? null : _versions.last;
      final page = await widget.bridge.listBindingVersions(
        bindingId: widget.entry.bindingId,
        pathKey: widget.entry.pathKey,
        afterArchivedAt: last?.archivedAt,
        afterVersionId: last?.versionId,
        limit: _pageSize,
      );
      if (!mounted) return;
      setState(() {
        _versions = reset ? page : <CatalogVersionModel>[..._versions, ...page];
        _hasMore = page.length >= _pageSize;
        _loading = false;
        _loadingMore = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _loadingMore = false;
      });
      NeuToast.error(
        context,
        '加载文件历史版本失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _restoreVersion(CatalogVersionModel version) async {
    if (_restoringVersionId != null) return;
    final targetDir = await widget.bridge.pickDirectory(
      description: '选择安全恢复目标目录（避免覆盖原同步目录）',
    );
    if (targetDir == null || !mounted) return;

    setState(() => _restoringVersionId = version.versionId);
    try {
      final result = await widget.bridge.restoreCatalogVersion(
        bindingId: version.bindingId,
        versionId: version.versionId,
        targetDir: targetDir,
      );
      if (!mounted) return;
      NeuToast.success(
        context,
        '已恢复至 ${result.outputPath}（${SystemEnvHelper.formatBytes(result.restoredBytes)}）',
      );
      Navigator.of(context).pop(result);
    } catch (err) {
      if (!mounted) return;
      setState(() => _restoringVersionId = null);
      NeuToast.error(
        context,
        '恢复归档版本失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  String _formatTs(int secs) {
    if (secs <= 0) return '-';
    final dt = DateTime.fromMillisecondsSinceEpoch(secs * 1000);
    return '${dt.year}-${dt.month.toString().padLeft(2, '0')}-${dt.day.toString().padLeft(2, '0')} '
        '${dt.hour.toString().padLeft(2, '0')}:${dt.minute.toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return NeuDialog(
      title: '历史版本 · ${widget.entry.displayPath}',
      subtitle: '镜像历史版本默认恢复到您指定的安全目录，避免在下次镜像同步时被覆盖。',
      maxWidth: 520,
      actions: <Widget>[
        NeuButton(
          label: '关闭',
          semanticLabel: '关闭文件历史版本弹窗',
          onPressed: _restoringVersionId != null
              ? null
              : () => Navigator.of(context).pop(),
        ),
      ],
      child: _loading && _versions.isEmpty
          ? Padding(
              padding: const EdgeInsets.symmetric(vertical: 16),
              child: Center(
                child: Text('正在加载版本历史…', style: textTheme.bodySmall),
              ),
            )
          : _versions.isEmpty
              ? NeuCard(
                  inset: true,
                  padding: const EdgeInsets.all(16),
                  child: Text('暂无可恢复的历史归档版本。', style: textTheme.bodySmall),
                )
              : Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: <Widget>[
                    for (var i = 0; i < _versions.length; i++) ...<Widget>[
                      Builder(
                        builder: (context) {
                          final v = _versions[i];
                          final isRestoring =
                              _restoringVersionId == v.versionId;
                          return Padding(
                            padding: const EdgeInsets.symmetric(vertical: 8),
                            child: Row(
                              children: <Widget>[
                                Expanded(
                                  child: Column(
                                    crossAxisAlignment:
                                        CrossAxisAlignment.start,
                                    children: <Widget>[
                                      Wrap(
                                        spacing: 8,
                                        runSpacing: 4,
                                        crossAxisAlignment:
                                            WrapCrossAlignment.center,
                                        children: <Widget>[
                                          Text(
                                            _formatTs(v.archivedAt),
                                            style: textTheme.titleSmall,
                                          ),
                                          NeuStatusBadge(
                                            label: v.archiveReason,
                                            tone: NeuStatusTone.accent,
                                            showDot: false,
                                          ),
                                          if (v.isRecycled)
                                            const NeuStatusBadge(
                                              label: '已回收',
                                              tone: NeuStatusTone.neutral,
                                              showDot: false,
                                            ),
                                        ],
                                      ),
                                      const SizedBox(height: 2),
                                      Text(
                                        '${SystemEnvHelper.formatBytes(v.size)} · 版本 ${v.versionId}',
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        style: textTheme.bodySmall,
                                      ),
                                    ],
                                  ),
                                ),
                                const SizedBox(width: 8),
                                NeuButton(
                                  label: '恢复到目录',
                                  icon: Icons.restore_rounded,
                                  tone: NeuButtonTone.primary,
                                  loading: isRestoring,
                                  semanticLabel: '恢复版本 ${v.versionId} 到指定目录',
                                  onPressed: v.isRecycled ||
                                          _restoringVersionId != null
                                      ? null
                                      : () => _restoreVersion(v),
                                ),
                              ],
                            ),
                          );
                        },
                      ),
                      if (i < _versions.length - 1) const Divider(height: 1),
                    ],
                    if (_hasMore) ...<Widget>[
                      const SizedBox(height: 10),
                      Center(
                        child: NeuButton(
                          label: '加载更早版本',
                          icon: Icons.expand_more_rounded,
                          loading: _loadingMore,
                          semanticLabel: '加载更早的历史版本',
                          onPressed: () => _loadVersions(reset: false),
                        ),
                      ),
                    ],
                  ],
                ),
    );
  }
}

class _ScrubMetricTile extends StatelessWidget {
  const _ScrubMetricTile({
    required this.label,
    required this.value,
    required this.color,
  });

  final String label;
  final String value;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: NeuTokens.insetSurface,
        borderRadius: const BorderRadius.all(Radius.circular(12)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text(
            value,
            style: Theme.of(context).textTheme.headlineSmall?.copyWith(
              fontSize: 20,
              color: color,
              fontFeatures: const <FontFeature>[
                FontFeature.tabularFigures(),
              ],
            ),
          ),
          const SizedBox(height: 2),
          Text(label, style: Theme.of(context).textTheme.bodySmall),
        ],
      ),
    );
  }
}
