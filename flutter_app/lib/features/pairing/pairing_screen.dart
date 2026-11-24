import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

enum _DeviceTab { paired, discovered }

/// 2. 设备页：
/// - 右上角蓝色 `+ 配对新设备` 按钮，弹窗整合本机 6 位配对 PIN 信息（get_pairing_info）与输入配对表单
/// - Segmented 标签切换“我的设备 (N)”与“局域网发现 (N)”
/// - 完整支持已配对设备解绑（revoke_paired_peer）与局域网 mDNS 设备发现一键配对
/// - 设备组管理（创建/重命名/删除设备组、查看与增删成员）
class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final Map<String, DiscoveredPeer> _discovered = <String, DiscoveredPeer>{};
  List<DeviceGroupViewModel> _deviceGroups = const <DeviceGroupViewModel>[];
  StreamSubscription<DiscoveredPeer>? _scanSub;
  Timer? _refreshTimer;
  Timer? _scanTimeoutTimer;

  _DeviceTab _activeTab = _DeviceTab.paired;
  bool _scanning = false;
  bool _loadingPaired = false;
  bool _loadingGroups = false;

  @override
  void initState() {
    super.initState();
    _refreshPairedPeers();
    _refreshDeviceGroups();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // 定期刷新已配对列表与设备组，让"在线/离线"随握手租约（45s）及时收敛，
    // 用户不需要切页或重启就能看到两端状态一致。
    _refreshTimer ??= Timer.periodic(
      const Duration(seconds: 5),
      (_) {
        unawaited(_refreshPairedPeers(silent: true));
        unawaited(_refreshDeviceGroups(silent: true));
      },
    );
  }

  @override
  void dispose() {
    _refreshTimer?.cancel();
    _refreshTimer = null;
    _scanTimeoutTimer?.cancel();
    _scanTimeoutTimer = null;
    _scanSub?.cancel();
    _scanSub = null;
    super.dispose();
  }

  void _stopScan({bool silent = true}) {
    _scanTimeoutTimer?.cancel();
    _scanTimeoutTimer = null;
    _scanSub?.cancel();
    _scanSub = null;
    if (_scanning && mounted) {
      setState(() => _scanning = false);
      if (!silent) {
        NeuToast.info(context, '已停止局域网设备扫描');
      }
    } else {
      _scanning = false;
    }
  }

  Future<void> _refreshPairedPeers({bool silent = false}) async {
    if (_loadingPaired) return;
    if (!silent) setState(() => _loadingPaired = true);
    try {
      await widget.bridge.refreshPairedPeers();
      if (!mounted) return;
      setState(() => _loadingPaired = false);
    } catch (err) {
      if (!mounted) return;
      setState(() => _loadingPaired = false);
      if (!silent) {
        NeuToast.error(
          context,
          '读取已配对设备失败：${LanVaultBridge.formatUserError(err)}',
        );
      }
    }
  }

  Future<void> _refreshDeviceGroups({bool silent = false}) async {
    if (_loadingGroups) return;
    if (!silent) setState(() => _loadingGroups = true);
    try {
      final groups = await widget.bridge.listDeviceGroups();
      if (!mounted) return;
      setState(() {
        _deviceGroups = groups;
        _loadingGroups = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loadingGroups = false);
      if (!silent) {
        NeuToast.error(
          context,
          '读取设备组失败：${LanVaultBridge.formatUserError(err)}',
        );
      }
    }
  }

  void _toggleScan() {
    if (_scanning) {
      _stopScan(silent: false);
      return;
    }
    _scanTimeoutTimer?.cancel();
    _scanTimeoutTimer = Timer(const Duration(seconds: 20), () {
      if (!mounted || !_scanning) return;
      _stopScan(silent: true);
      NeuToast.info(context, '局域网扫描已完成，如需继续可点击重新扫描');
    });

    setState(() {
      _scanning = true;
      _activeTab = _DeviceTab.discovered;
    });
    NeuToast.info(context, '正在扫描局域网内的 EveryoneNAS 设备…');

    try {
      _scanSub = widget.bridge.startMdnsScan().listen(
        (peer) {
          if (!mounted) return;
          setState(() => _discovered[peer.deviceId] = peer);
        },
        onError: (Object err) {
          _stopScan(silent: true);
          if (!mounted) return;
          NeuToast.error(
              context, '扫描失败：${LanVaultBridge.formatUserError(err)}');
        },
        onDone: () {
          _stopScan(silent: true);
        },
        cancelOnError: true,
      );
    } catch (err) {
      _stopScan(silent: true);
      if (mounted) {
        NeuToast.error(
            context, '启动扫描失败：${LanVaultBridge.formatUserError(err)}');
      }
    }
  }

  Future<void> _revokePeer(PairedPeerModel peer) async {
    try {
      final revoked = await widget.bridge.revokePairedPeer(peer.deviceId);
      if (!mounted) return;
      if (revoked) {
        NeuToast.success(context, '已解绑设备 ${peer.deviceName}');
      } else {
        NeuToast.warning(context, '未能解绑设备，请稍后重试');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '解绑失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _openPairDialog({String? initialIp, int? initialPort}) async {
    final paired = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) => _PairNewDeviceDialog(
        bridge: widget.bridge,
        initialIp: initialIp,
        initialPort: initialPort,
      ),
    );
    if (paired == true && mounted) {
      _stopScan(silent: true);
      setState(() => _activeTab = _DeviceTab.paired);
      await _refreshPairedPeers();
    }
  }

  Future<void> _openCreateGroupDialog() async {
    final controller = TextEditingController();
    final createdName = await showNeuDialog<String>(
      context: context,
      builder: (dialogContext) {
        return NeuDialog(
          title: '新建设备组',
          subtitle: '将多台已配对设备编入同一组，用于统一配置差异备份与镜像同步规则。',
          maxWidth: 440,
          actions: <Widget>[
            NeuButton(
              label: '取消',
              semanticLabel: '取消新建设备组',
              onPressed: () => Navigator.of(dialogContext).pop(),
            ),
            NeuButton(
              label: '创建设备组',
              icon: Icons.check_rounded,
              tone: NeuButtonTone.primary,
              semanticLabel: '确认创建设备组',
              onPressed: () {
                final name = controller.text.trim();
                if (name.isEmpty) {
                  NeuToast.warning(dialogContext, '请输入设备组名称');
                  return;
                }
                Navigator.of(dialogContext).pop(name);
              },
            ),
          ],
          child: NeuInsetField(
            controller: controller,
            label: '设备组名称',
            hint: '例如：家庭工作站、工作室备份组',
            prefixIcon: Icons.group_work_outlined,
            onSubmitted: (val) {
              final trimmed = val.trim();
              if (trimmed.isNotEmpty) {
                Navigator.of(dialogContext).pop(trimmed);
              }
            },
          ),
        );
      },
    );
    controller.dispose();
    if (createdName == null || !mounted) return;

    try {
      await widget.bridge.createDeviceGroup(createdName);
      if (!mounted) return;
      NeuToast.success(context, '已创建设备组「$createdName」');
      await _refreshDeviceGroups();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '创建设备组失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _openRenameGroupDialog(DeviceGroupModel group) async {
    final controller = TextEditingController(text: group.name);
    final nextName = await showNeuDialog<String>(
      context: context,
      builder: (dialogContext) {
        return NeuDialog(
          title: '重命名设备组',
          maxWidth: 420,
          actions: <Widget>[
            NeuButton(
              label: '取消',
              semanticLabel: '取消重命名设备组',
              onPressed: () => Navigator.of(dialogContext).pop(),
            ),
            NeuButton(
              label: '保存名称',
              icon: Icons.check_rounded,
              tone: NeuButtonTone.primary,
              semanticLabel: '保存设备组新名称',
              onPressed: () {
                final name = controller.text.trim();
                if (name.isEmpty) {
                  NeuToast.warning(dialogContext, '请输入新的设备组名称');
                  return;
                }
                Navigator.of(dialogContext).pop(name);
              },
            ),
          ],
          child: NeuInsetField(
            controller: controller,
            label: '新名称',
            hint: '输入新的设备组名称',
            prefixIcon: Icons.edit_outlined,
            onSubmitted: (val) {
              final trimmed = val.trim();
              if (trimmed.isNotEmpty) {
                Navigator.of(dialogContext).pop(trimmed);
              }
            },
          ),
        );
      },
    );
    controller.dispose();
    if (nextName == null || nextName == group.name || !mounted) return;

    try {
      await widget.bridge.renameDeviceGroup(
        groupId: group.groupId,
        name: nextName,
      );
      if (!mounted) return;
      NeuToast.success(context, '设备组已重命名为「$nextName」');
      await _refreshDeviceGroups();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '重命名失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _confirmDeleteGroup(DeviceGroupModel group) async {
    final confirmed = await showNeuDialog<bool>(
      context: context,
      builder: (dialogContext) {
        return NeuDialog(
          title: '删除设备组「${group.name}」',
          subtitle: '请确认是否删除此设备组',
          maxWidth: 440,
          actions: <Widget>[
            NeuButton(
              label: '取消',
              semanticLabel: '取消删除设备组',
              onPressed: () => Navigator.of(dialogContext).pop(false),
            ),
            NeuButton(
              label: '确认删除',
              icon: Icons.delete_outline_rounded,
              tone: NeuButtonTone.danger,
              semanticLabel: '确认删除设备组 ${group.name}',
              onPressed: () => Navigator.of(dialogContext).pop(true),
            ),
          ],
          child: NeuCard(
            inset: true,
            padding: const EdgeInsets.all(14),
            child: Text(
              '删除设备组后，该组关联的所有同步与镜像规则将被自动停用，不会继续自动执行；但已有备份数据与历史版本归档将完整保留，仍可随时执行恢复。',
              style: Theme.of(dialogContext).textTheme.bodyMedium,
            ),
          ),
        );
      },
    );
    if (confirmed != true || !mounted) return;

    try {
      await widget.bridge.deleteDeviceGroup(group.groupId);
      if (!mounted) return;
      NeuToast.info(context, '已删除设备组「${group.name}」，关联规则已停用');
      await _refreshDeviceGroups();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '删除设备组失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _openAddMemberDialog(
    DeviceGroupViewModel groupView,
    List<PairedPeerModel> pairedPeers,
  ) async {
    final activeIds = groupView.activeMembers.map((m) => m.deviceId).toSet();
    final candidates = pairedPeers
        .where((p) => !activeIds.contains(p.deviceId))
        .toList(growable: false);
    if (candidates.isEmpty) {
      NeuToast.info(context, '所有已配对设备均已在该设备组中');
      return;
    }

    String? selectedDeviceId = candidates.first.deviceId;
    final pickedDeviceId = await showNeuDialog<String>(
      context: context,
      builder: (dialogContext) {
        return StatefulBuilder(
          builder: (context, setDialogState) {
            return NeuDialog(
              title: '添加成员到「${groupView.group.name}」',
              maxWidth: 440,
              actions: <Widget>[
                NeuButton(
                  label: '取消',
                  semanticLabel: '取消添加成员',
                  onPressed: () => Navigator.of(dialogContext).pop(),
                ),
                NeuButton(
                  label: '添加成员',
                  icon: Icons.person_add_alt_1_rounded,
                  tone: NeuButtonTone.primary,
                  semanticLabel: '确认添加成员到设备组',
                  onPressed: selectedDeviceId == null
                      ? null
                      : () => Navigator.of(dialogContext).pop(selectedDeviceId),
                ),
              ],
              child: NeuDropdown<String>(
                label: '选择已配对设备',
                prefixIcon: Icons.devices_outlined,
                value: selectedDeviceId,
                options: candidates
                    .map(
                      (peer) => NeuDropdownOption<String>(
                        value: peer.deviceId,
                        label: peer.displayLabel,
                        subtitle: peer.presenceLabel,
                      ),
                    )
                    .toList(growable: false),
                onChanged: (val) =>
                    setDialogState(() => selectedDeviceId = val),
              ),
            );
          },
        );
      },
    );

    if (pickedDeviceId == null || !mounted) return;
    try {
      await widget.bridge.addGroupMember(
        groupId: groupView.group.groupId,
        deviceId: pickedDeviceId,
        role: 'member',
      );
      if (!mounted) return;
      NeuToast.success(context, '已添加成员到「${groupView.group.name}」');
      await _refreshDeviceGroups();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '添加成员失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _removeMember(
    DeviceGroupModel group,
    GroupMemberModel member,
    String displayName,
  ) async {
    try {
      await widget.bridge.removeGroupMember(
        groupId: group.groupId,
        deviceId: member.deviceId,
      );
      if (!mounted) return;
      NeuToast.info(context, '已从「${group.name}」移除 $displayName');
      await _refreshDeviceGroups();
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(
        context,
        '移除成员失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  String _resolveDeviceLabel(
    String deviceId,
    List<PairedPeerModel> pairedPeers, {
    bool isCoordinator = false,
  }) {
    for (final peer in pairedPeers) {
      if (peer.deviceId == deviceId) {
        return peer.deviceName.trim().isEmpty
            ? peer.deviceId
            : peer.deviceName.trim();
      }
    }
    if (isCoordinator) {
      return '本机设备 ($deviceId)';
    }
    return deviceId;
  }

  (IconData, Color, Color) _deviceIcon(String osType) {
    final lower = osType.toLowerCase();
    if (lower.contains('android')) {
      return (
        Icons.android_rounded,
        NeuTokens.healthy,
        NeuTokens.healthySoft,
      );
    }
    if (lower.contains('windows') || lower.contains('win')) {
      return (
        Icons.window_rounded,
        NeuTokens.accent,
        NeuTokens.accentSoft,
      );
    }
    return (
      Icons.laptop_mac_rounded,
      NeuTokens.slateMuted,
      NeuTokens.insetSurface,
    );
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
    final discoveredList = _discovered.values.toList(growable: false);

    return ValueListenableBuilder<List<PairedPeerModel>>(
      valueListenable: widget.bridge.pairedPeersNotifier,
      builder: (context, pairedPeers, _) {
        final segmentControl = NeuSegmentedControl<_DeviceTab>(
          semanticLabel: '设备列表分类',
          value: _activeTab,
          onChanged: (tab) {
            if (tab == _DeviceTab.paired && _scanning) {
              _stopScan(silent: true);
            }
            setState(() => _activeTab = tab);
          },
          segments: <NeuSegment<_DeviceTab>>[
            NeuSegment<_DeviceTab>(
              value: _DeviceTab.paired,
              label: '我的设备 (${pairedPeers.length})',
            ),
            NeuSegment<_DeviceTab>(
              value: _DeviceTab.discovered,
              label: '局域网发现 (${discoveredList.length})',
            ),
          ],
        );
        final scanButton = NeuButton(
          label: _scanning ? '停止扫描' : '扫描局域网',
          icon: _scanning ? Icons.stop_rounded : Icons.radar_rounded,
          tone: _scanning ? NeuButtonTone.danger : NeuButtonTone.neutral,
          expanded: !isWide,
          onPressed: _toggleScan,
        );

        return SingleChildScrollView(
          padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: <Widget>[
                  if (isWide)
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Text('设备', style: textTheme.headlineSmall),
                          const SizedBox(height: 4),
                          Text(
                            '管理已配对的设备、局域网发现与设备组',
                            style: textTheme.bodySmall,
                          ),
                        ],
                      ),
                    )
                  else
                    Expanded(
                      child: Text(
                        '管理已配对设备与设备组',
                        style: textTheme.bodySmall,
                      ),
                    ),
                  const SizedBox(width: NeuSpace.sm),
                  NeuButton(
                    label: '配对新设备',
                    icon: Icons.add_rounded,
                    tone: NeuButtonTone.primary,
                    semanticLabel: '配对新设备',
                    onPressed: () => _openPairDialog(),
                  ),
                ],
              ),
              const SizedBox(height: 18),
              if (isWide)
                Row(
                  children: <Widget>[
                    Expanded(
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 380),
                        child: segmentControl,
                      ),
                    ),
                    if (_activeTab == _DeviceTab.discovered) ...<Widget>[
                      const SizedBox(width: NeuSpace.sm),
                      scanButton,
                    ],
                  ],
                )
              else ...<Widget>[
                segmentControl,
                if (_activeTab == _DeviceTab.discovered) ...<Widget>[
                  const SizedBox(height: NeuSpace.sm),
                  scanButton,
                ],
              ],
              const SizedBox(height: NeuSpace.md),
              if (_activeTab == _DeviceTab.paired)
                _buildPairedListCard(context, pairedPeers)
              else
                _buildDiscoveredListCard(context, discoveredList),
              const SizedBox(height: NeuSpace.lg),
              _buildDeviceGroupsSection(context, pairedPeers, isWide),
            ],
          ),
        );
      },
    );
  }

  Widget _buildDeviceGroupsSection(
    BuildContext context,
    List<PairedPeerModel> pairedPeers,
    bool isWide,
  ) {
    final textTheme = Theme.of(context).textTheme;
    return Semantics(
      container: true,
      label: '设备组管理',
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
                    Row(
                      mainAxisSize: MainAxisSize.min,
                      children: <Widget>[
                        Text('设备组管理', style: textTheme.titleMedium),
                        const SizedBox(width: 8),
                        NeuStatusBadge(
                          label: '${_deviceGroups.length} 个组',
                          tone: NeuStatusTone.accent,
                          showDot: false,
                        ),
                      ],
                    ),
                    const SizedBox(height: 2),
                    Text(
                      '多设备协同分组，用于配置一对多差异备份与镜像同步规则',
                      style: textTheme.bodySmall,
                    ),
                  ],
                ),
                Wrap(
                  spacing: NeuSpace.sm,
                  runSpacing: NeuSpace.xs,
                  children: <Widget>[
                    NeuButton(
                      label: '刷新',
                      icon: Icons.refresh_rounded,
                      semanticLabel: '刷新设备组列表',
                      loading: _loadingGroups,
                      onPressed: () => _refreshDeviceGroups(),
                    ),
                    NeuButton(
                      label: '新建设备组',
                      icon: Icons.group_add_rounded,
                      tone: NeuButtonTone.primary,
                      semanticLabel: '新建设备组',
                      onPressed: _openCreateGroupDialog,
                    ),
                  ],
                ),
              ],
            ),
            const SizedBox(height: 16),
            if (_loadingGroups && _deviceGroups.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 16),
                child: Center(
                  child: Text('正在加载设备组…', style: textTheme.bodySmall),
                ),
              )
            else if (_deviceGroups.isEmpty)
              const NeuEmptyState(
                icon: Icons.workspaces_outline,
                title: '暂无设备组',
                subtitle: '点击“新建设备组”将本机与已配对设备编组，便于配置多目标同步规则。',
              )
            else
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  for (var i = 0; i < _deviceGroups.length; i++) ...<Widget>[
                    _buildSingleGroupCard(
                      context,
                      _deviceGroups[i],
                      pairedPeers,
                      isWide,
                    ),
                    if (i < _deviceGroups.length - 1)
                      const SizedBox(height: 12),
                  ],
                ],
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildSingleGroupCard(
    BuildContext context,
    DeviceGroupViewModel groupView,
    List<PairedPeerModel> pairedPeers,
    bool isWide,
  ) {
    final textTheme = Theme.of(context).textTheme;
    final group = groupView.group;
    final activeMembers = groupView.activeMembers;

    return Semantics(
      container: true,
      label: '设备组 ${group.name}，共 ${activeMembers.length} 名成员',
      child: NeuCard(
        key: ValueKey<String>('group_${group.groupId}'),
        inset: true,
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Wrap(
              spacing: NeuSpace.sm,
              runSpacing: NeuSpace.sm,
              alignment: WrapAlignment.spaceBetween,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: <Widget>[
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: <Widget>[
                    const NeuIconBadge(
                      icon: Icons.workspaces_rounded,
                      color: NeuTokens.accent,
                      backgroundColor: NeuTokens.accentSoft,
                      size: 38,
                      iconSize: 20,
                    ),
                    const SizedBox(width: 12),
                    Flexible(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: <Widget>[
                          Wrap(
                            spacing: 8,
                            runSpacing: 4,
                            crossAxisAlignment: WrapCrossAlignment.center,
                            children: <Widget>[
                              Text(group.name, style: textTheme.titleSmall),
                              NeuStatusBadge(
                                label: '${activeMembers.length} 台设备',
                                tone: NeuStatusTone.neutral,
                                showDot: false,
                              ),
                              NeuStatusBadge(
                                label: 'Rev ${group.revision}',
                                tone: NeuStatusTone.accent,
                                showDot: false,
                              ),
                            ],
                          ),
                          const SizedBox(height: 2),
                          Text(
                            '组 ID：${group.groupId}',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: textTheme.bodySmall,
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
                Wrap(
                  spacing: NeuSpace.xs,
                  runSpacing: NeuSpace.xs,
                  children: <Widget>[
                    NeuButton(
                      label: '添加成员',
                      icon: Icons.person_add_alt_1_rounded,
                      semanticLabel: '向设备组 ${group.name} 添加成员',
                      onPressed: pairedPeers.isEmpty
                          ? null
                          : () => _openAddMemberDialog(groupView, pairedPeers),
                    ),
                    NeuButton(
                      label: '重命名',
                      icon: Icons.edit_outlined,
                      semanticLabel: '重命名设备组 ${group.name}',
                      onPressed: () => _openRenameGroupDialog(group),
                    ),
                    NeuButton(
                      label: '删除',
                      icon: Icons.delete_outline_rounded,
                      tone: NeuButtonTone.danger,
                      semanticLabel: '删除设备组 ${group.name}',
                      onPressed: () => _confirmDeleteGroup(group),
                    ),
                  ],
                ),
              ],
            ),
            const SizedBox(height: 12),
            const Divider(height: 1),
            const SizedBox(height: 10),
            if (activeMembers.isEmpty)
              Text('暂无活跃成员', style: textTheme.bodySmall)
            else
              Column(
                children: <Widget>[
                  for (var idx = 0;
                      idx < activeMembers.length;
                      idx++) ...<Widget>[
                    Builder(
                      builder: (context) {
                        final member = activeMembers[idx];
                        final memberLabel = _resolveDeviceLabel(
                          member.deviceId,
                          pairedPeers,
                          isCoordinator: member.isCoordinator,
                        );
                        return Padding(
                          padding: const EdgeInsets.symmetric(vertical: 6),
                          child: Row(
                            children: <Widget>[
                              Icon(
                                member.isCoordinator
                                    ? Icons.star_rounded
                                    : Icons.devices_rounded,
                                size: 18,
                                color: member.isCoordinator
                                    ? NeuTokens.warning
                                    : NeuTokens.accent,
                              ),
                              const SizedBox(width: 10),
                              Expanded(
                                child: Text(
                                  memberLabel,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: textTheme.bodyMedium,
                                ),
                              ),
                              const SizedBox(width: 8),
                              NeuStatusBadge(
                                label: member.isCoordinator ? '协调者' : '成员',
                                tone: member.isCoordinator
                                    ? NeuStatusTone.warning
                                    : NeuStatusTone.neutral,
                                showDot: false,
                              ),
                              if (!member.isCoordinator) ...<Widget>[
                                const SizedBox(width: 8),
                                Semantics(
                                  button: true,
                                  label: '从设备组 ${group.name} 移除成员 $memberLabel',
                                  child: NeuButton(
                                    label: '移除',
                                    tone: NeuButtonTone.danger,
                                    semanticLabel:
                                        '从设备组 ${group.name} 移除成员 $memberLabel',
                                    onPressed: () => _removeMember(
                                      group,
                                      member,
                                      memberLabel,
                                    ),
                                  ),
                                ),
                              ],
                            ],
                          ),
                        );
                      },
                    ),
                    if (idx < activeMembers.length - 1)
                      const Divider(height: 1),
                  ],
                ],
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildPairedListCard(
    BuildContext context,
    List<PairedPeerModel> peers,
  ) {
    final textTheme = Theme.of(context).textTheme;
    if (_loadingPaired && peers.isEmpty) {
      return NeuCard(
        padding: const EdgeInsets.all(24),
        child: Center(
          child: Text('正在加载已配对设备…', style: textTheme.bodySmall),
        ),
      );
    }
    if (peers.isEmpty) {
      return const NeuEmptyState(
        icon: Icons.devices_other_rounded,
        title: '暂无已配对设备',
        subtitle: '点击“扫描局域网”发现设备发起配对，或点击“+ 配对新设备”查看本机配对信息。',
      );
    }

    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 8),
      child: Column(
        children: <Widget>[
          for (var i = 0; i < peers.length; i++) ...<Widget>[
            Builder(
              key: ValueKey<String>('paired_${peers[i].deviceId}'),
              builder: (context) {
                final peer = peers[i];
                final (icon, fg, bg) = _deviceIcon(peer.osType);
                final hasIp = peer.ip != null && peer.ip!.isNotEmpty;
                return InkWell(
                  onTap: () {
                    widget.bridge.selectPeer(peer.deviceId);
                    NeuToast.success(context, '已切换当前目标设备为 ${peer.deviceName}');
                  },
                  borderRadius: const BorderRadius.all(Radius.circular(12)),
                  child: Padding(
                    padding:
                        const EdgeInsets.symmetric(vertical: 12, horizontal: 4),
                    child: Row(
                      children: <Widget>[
                        NeuIconBadge(
                          icon: icon,
                          color: fg,
                          backgroundColor: bg,
                          size: 44,
                          iconSize: 22,
                        ),
                        const SizedBox(width: 14),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                peer.deviceName.trim().isEmpty
                                    ? peer.deviceId
                                    : peer.deviceName,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.titleSmall?.copyWith(
                                  fontSize: 15,
                                ),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                hasIp
                                    ? '${peer.ip}${peer.port != null ? ':${peer.port}' : ''}'
                                    : peer.deviceId,
                                overflow: TextOverflow.ellipsis,
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 8),
                        NeuStatusBadge(
                          label: peer.presenceLabel,
                          tone: peer.online
                              ? NeuStatusTone.healthy
                              : NeuStatusTone.neutral,
                        ),
                        const SizedBox(width: 4),
                        PopupMenuButton<String>(
                          tooltip: '设备操作',
                          icon: const Icon(
                            Icons.more_horiz_rounded,
                            color: NeuTokens.textSecondary,
                          ),
                          onSelected: (value) {
                            if (value == 'revoke') {
                              _revokePeer(peer);
                            }
                          },
                          itemBuilder: (context) =>
                              const <PopupMenuEntry<String>>[
                            PopupMenuItem<String>(
                              value: 'revoke',
                              child: Row(
                                children: <Widget>[
                                  Icon(
                                    Icons.link_off_rounded,
                                    size: 18,
                                    color: NeuTokens.danger,
                                  ),
                                  SizedBox(width: 8),
                                  Text(
                                    '解绑设备',
                                    style: TextStyle(color: NeuTokens.danger),
                                  ),
                                ],
                              ),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                );
              },
            ),
            if (i < peers.length - 1) const Divider(height: 1),
          ],
        ],
      ),
    );
  }

  Widget _buildDiscoveredListCard(
    BuildContext context,
    List<DiscoveredPeer> peers,
  ) {
    final textTheme = Theme.of(context).textTheme;
    if (peers.isEmpty) {
      return NeuEmptyState(
        icon: Icons.radar_rounded,
        title: _scanning ? '正在搜索局域网设备…' : '尚未发现局域网设备',
        subtitle: _scanning
            ? '请确保目标设备的 EveryoneNAS 处于同一局域网并已运行。'
            : '点击“扫描局域网”以通过 mDNS 发现周边 Windows / Android 设备。',
      );
    }

    return NeuCard(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 8),
      child: Column(
        children: <Widget>[
          for (var i = 0; i < peers.length; i++) ...<Widget>[
            Builder(
              key: ValueKey<String>('discovered_${peers[i].deviceId}'),
              builder: (context) {
                final peer = peers[i];
                final (icon, fg, bg) = _deviceIcon(peer.osType);
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 12),
                  child: Row(
                    children: <Widget>[
                      NeuIconBadge(
                        icon: icon,
                        color: fg,
                        backgroundColor: bg,
                        size: 44,
                        iconSize: 22,
                      ),
                      const SizedBox(width: 14),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              peer.deviceName,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: textTheme.titleSmall?.copyWith(
                                fontSize: 15,
                              ),
                            ),
                            const SizedBox(height: 2),
                            Text(
                              '${peer.ip}:${peer.port} · 指纹 ${peer.fingerprint}',
                              overflow: TextOverflow.ellipsis,
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 10),
                      NeuButton(
                        label: '配对',
                        icon: Icons.link_rounded,
                        tone: NeuButtonTone.primary,
                        onPressed: () => _openPairDialog(
                            initialIp: peer.ip, initialPort: peer.port),
                      ),
                    ],
                  ),
                );
              },
            ),
            if (i < peers.length - 1) const Divider(height: 1),
          ],
        ],
      ),
    );
  }
}

class _PairNewDeviceDialog extends StatefulWidget {
  const _PairNewDeviceDialog({
    required this.bridge,
    this.initialIp,
    this.initialPort,
  });

  final LanVaultBridge bridge;
  final String? initialIp;
  final int? initialPort;

  @override
  State<_PairNewDeviceDialog> createState() => _PairNewDeviceDialogState();
}

class _PairNewDeviceDialogState extends State<_PairNewDeviceDialog> {
  late final TextEditingController _ipController = TextEditingController(
    text: widget.initialIp ?? '',
  );
  late final TextEditingController _portController = TextEditingController(
    text: (widget.initialPort ?? LanVaultBridge.defaultServerPort).toString(),
  );
  final TextEditingController _pinController = TextEditingController();
  final TextEditingController _fingerprintController = TextEditingController();

  PairingInfoModel? _localInfo;
  bool _loadingInfo = true;
  bool _pairing = false;

  @override
  void initState() {
    super.initState();
    _pinController.addListener(_onPinChanged);
    _loadLocalPairingInfo();
  }

  void _onPinChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _pinController.removeListener(_onPinChanged);
    _ipController.dispose();
    _portController.dispose();
    _pinController.dispose();
    _fingerprintController.dispose();
    super.dispose();
  }

  Future<void> _loadLocalPairingInfo() async {
    setState(() => _loadingInfo = true);
    try {
      final info = await widget.bridge.getPairingInfo();
      if (!mounted) return;
      setState(() {
        _localInfo = info;
        _loadingInfo = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loadingInfo = false);
      NeuToast.error(
        context,
        '读取本机配对信息失败：${LanVaultBridge.formatUserError(err)}',
      );
    }
  }

  Future<void> _submitInteractiveRequest(String address, int port) async {
    setState(() => _pairing = true);
    try {
      NeuToast.info(context, '请在对方设备确认配对');
      final pairedId =
          await widget.bridge.requestPairing(ip: address, port: port);
      if (!mounted) return;
      NeuToast.success(context, '配对成功（设备：$pairedId）');
      Navigator.of(context).pop(true);
    } catch (err) {
      if (!mounted) return;
      setState(() => _pairing = false);
      NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _submitPair() async {
    final ip = _ipController.text.trim();
    final port = int.tryParse(_portController.text.trim());
    final pin = _pinController.text.trim();
    final fingerprint = _fingerprintController.text.trim();

    if (ip.isEmpty || port == null || port <= 0 || port > 65535) {
      NeuToast.warning(context, '请输入有效的目标 IP 与端口（1–65535）');
      return;
    }
    if (!RegExp(r'^[0-9a-fA-F]{32}$').hasMatch(fingerprint)) {
      NeuToast.warning(context, '请从对方设备复制 32 位十六进制指纹');
      return;
    }
    final address = '$ip#$fingerprint';
    if (pin.isEmpty) {
      await _submitInteractiveRequest(address, port);
      return;
    }
    if (!RegExp(r'^\d{6}$').hasMatch(pin)) {
      NeuToast.warning(context, '请输入 6 位数字配对 PIN');
      return;
    }

    setState(() => _pairing = true);
    try {
      final pairedId = await widget.bridge.pairWithPin(
        ip: address,
        port: port,
        pin: pin,
      );
      if (!mounted) return;
      NeuToast.success(context, '配对成功（设备：$pairedId）');
      Navigator.of(context).pop(true);
    } catch (err) {
      if (!mounted) return;
      setState(() => _pairing = false);
      NeuToast.error(context, '配对失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final info = _localInfo;
    final localFingerprint =
        Uri.tryParse(info?.pairingQrUri ?? '')?.queryParameters['fp'];

    return NeuDialog(
      title: '配对新设备',
      maxWidth: 480,
      actions: <Widget>[
        NeuButton(
          label: '取消',
          onPressed: _pairing ? null : () => Navigator.of(context).pop(false),
        ),
        NeuButton(
          label: _pinController.text.trim().isEmpty ? '发起配对请求' : '验证 PIN 并配对',
          icon: Icons.verified_user_outlined,
          tone: NeuButtonTone.primary,
          loading: _pairing,
          onPressed: _submitPair,
        ),
      ],
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          NeuCard(
            inset: true,
            padding: const EdgeInsets.all(14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: <Widget>[
                    Text('本机配对信息', style: textTheme.titleSmall),
                    NeuStatusBadge(
                      label: info?.serverRunning == true ? '等待连接' : '服务未启动',
                      tone: info?.serverRunning == true
                          ? NeuStatusTone.healthy
                          : NeuStatusTone.neutral,
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                if (_loadingInfo)
                  Text('正在获取本机 6 位配对 PIN…', style: textTheme.bodySmall)
                else if (info != null)
                  Row(
                    children: <Widget>[
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              info.pairingPin.isEmpty
                                  ? '------'
                                  : info.pairingPin.split('').join(' '),
                              style: textTheme.headlineSmall?.copyWith(
                                color: NeuTokens.accent,
                                letterSpacing: 3,
                                fontFeatures: const <FontFeature>[
                                  FontFeature.tabularFigures(),
                                ],
                              ),
                            ),
                            const SizedBox(height: 4),
                            Text(
                              '局域网地址：${info.lanIp}:${info.port}',
                              style: textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                      NeuButton(
                        label: '复制 PIN',
                        icon: Icons.copy_rounded,
                        onPressed: info.pairingPin.isEmpty
                            ? null
                            : () async {
                                await Clipboard.setData(
                                  ClipboardData(text: info.pairingPin),
                                );
                                if (context.mounted) {
                                  NeuToast.success(context, '已复制本机配对 PIN');
                                }
                              },
                      ),
                    ],
                  ),
                if (localFingerprint != null)
                  NeuButton(
                    label: '复制本机指纹',
                    icon: Icons.fingerprint,
                    onPressed: () async {
                      await Clipboard.setData(
                          ClipboardData(text: localFingerprint));
                      if (context.mounted) {
                        NeuToast.success(context, '已复制本机指纹');
                      }
                    },
                  ),
              ],
            ),
          ),
          const SizedBox(height: 18),
          Text('连接其他设备', style: textTheme.titleSmall),
          const SizedBox(height: 10),
          Row(
            children: <Widget>[
              Expanded(
                flex: 3,
                child: NeuInsetField(
                  controller: _ipController,
                  label: '对端局域网 IP',
                  hint: '192.168.1.100',
                  prefixIcon: Icons.lan_outlined,
                ),
              ),
              const SizedBox(width: NeuSpace.sm),
              Expanded(
                flex: 2,
                child: NeuInsetField(
                  controller: _portController,
                  label: '端口',
                  hint: '7842',
                  keyboardType: TextInputType.number,
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          NeuInsetField(
            controller: _fingerprintController,
            label: '对端设备指纹',
            hint: '从对方设备的配对信息复制',
            prefixIcon: Icons.fingerprint,
          ),
          const SizedBox(height: 12),
          NeuInsetField(
            controller: _pinController,
            label: '6 位动态 PIN（选填，留空则向对方发送确认弹窗）',
            hint: '留空即发起一键确认请求，或输入对端 6 位数字',
            keyboardType: TextInputType.number,
            prefixIcon: Icons.pin_outlined,
            onSubmitted: (_) => _submitPair(),
          ),
        ],
      ),
    );
  }
}
