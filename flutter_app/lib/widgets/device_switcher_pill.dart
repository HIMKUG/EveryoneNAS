import 'package:material_ui/material_ui.dart';

import '../bridge/lanvault_bridge.dart';
import '../theme/neu_theme.dart';

/// 顶栏全局设备切换胶囊（双端复用）：
/// - 桌面端位于窗口标题栏居中位置；
/// - 移动端位于页面顶部标题栏右侧/居中区域。
/// 点击弹出快速切换面板（移动端 Bottom Sheet / 桌面端居中轻量弹窗），
/// 底部固定提供“管理全部设备 / 配对新设备”入口。
class DeviceSwitcherPill extends StatelessWidget {
  const DeviceSwitcherPill({
    super.key,
    required this.bridge,
    required this.onOpenDeviceManager,
    this.compact = false,
  });

  final LanVaultBridge bridge;
  final VoidCallback onOpenDeviceManager;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return ValueListenableBuilder<List<PairedPeerModel>>(
      valueListenable: bridge.pairedPeersNotifier,
      builder: (context, peers, _) {
        return ValueListenableBuilder<String?>(
          valueListenable: bridge.selectedPeerIdNotifier,
          builder: (context, selectedId, _) {
            final active = bridge.selectedPeer;
            final bool hasDevice = active != null;
            final bool online = active?.online ?? false;
            final String label = hasDevice ? active.deviceName : '未连接设备';

            return Semantics(
              button: true,
              label: '当前设备：$label，点击切换或管理设备',
              child: Material(
                color: Colors.transparent,
                child: InkWell(
                  onTap: () => _openSwitcher(context, peers, active),
                  borderRadius:
                      const BorderRadius.all(Radius.circular(NeuRadius.pill)),
                  child: Container(
                    constraints: BoxConstraints(
                      maxWidth: compact ? 168 : 236,
                      minHeight: compact ? 34 : 30,
                    ),
                    padding: EdgeInsets.symmetric(
                      horizontal: compact ? 11 : 12,
                      vertical: compact ? 6 : 4,
                    ),
                    decoration: BoxDecoration(
                      color: neu.surface,
                      borderRadius: const BorderRadius.all(
                        Radius.circular(NeuRadius.pill),
                      ),
                      border: Border.all(
                        color: online
                            ? neu.healthy.withValues(alpha: 0.35)
                            : neu.hairlineBorder,
                      ),
                      boxShadow: neu.controlShadows,
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: <Widget>[
                        Container(
                          width: 7,
                          height: 7,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: !hasDevice
                                ? neu.textSecondary.withValues(alpha: 0.45)
                                : (online
                                    ? neu.healthy
                                    : neu.warning),
                          ),
                        ),
                        const SizedBox(width: 7),
                        Flexible(
                          child: Text(
                            label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style:
                                Theme.of(context).textTheme.labelMedium?.copyWith(
                                      color: neu.textPrimary,
                                      fontWeight: FontWeight.w600,
                                      fontSize: compact ? 12.5 : 12,
                                    ),
                          ),
                        ),
                        const SizedBox(width: 3),
                        Icon(
                          Icons.keyboard_arrow_down_rounded,
                          size: 16,
                          color: neu.textSecondary,
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            );
          },
        );
      },
    );
  }

  Future<void> _openSwitcher(
    BuildContext context,
    List<PairedPeerModel> peers,
    PairedPeerModel? active,
  ) async {
    final neu = NeuTheme.of(context);
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
    if (isWide) {
      await showNeuDialog<void>(
        context: context,
        builder: (dialogCtx) => NeuDialog(
          title: '切换目标设备',
          subtitle: '切换后传输对话与默认目标设备将同步更新',
          maxWidth: 420,
          actions: <Widget>[
            NeuButton(
              label: '管理设备与配对',
              icon: Icons.devices_rounded,
              tone: NeuButtonTone.primary,
              onPressed: () {
                Navigator.of(dialogCtx).pop();
                onOpenDeviceManager();
              },
            ),
          ],
          child: _DeviceListContent(
            peers: peers,
            activeId: active?.deviceId,
            onSelect: (peer) {
              bridge.selectPeer(peer.deviceId);
              Navigator.of(dialogCtx).pop();
            },
            onOpenManager: () {
              Navigator.of(dialogCtx).pop();
              onOpenDeviceManager();
            },
            showFooterButton: false,
          ),
        ),
      );
      return;
    }

    await showModalBottomSheet<void>(
      context: context,
      backgroundColor: neu.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(22)),
      ),
      builder: (sheetCtx) {
        return SafeArea(
          top: false,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 12, 20, 20),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: <Widget>[
                Center(
                  child: Container(
                    width: 36,
                    height: 4,
                    decoration: BoxDecoration(
                      color: neu.controlOutline,
                      borderRadius: const BorderRadius.all(
                        Radius.circular(NeuRadius.pill),
                      ),
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                Text(
                  '切换目标设备',
                  style: Theme.of(sheetCtx).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                ),
                const SizedBox(height: 12),
                _DeviceListContent(
                  peers: peers,
                  activeId: active?.deviceId,
                  onSelect: (peer) {
                    bridge.selectPeer(peer.deviceId);
                    Navigator.of(sheetCtx).pop();
                  },
                  onOpenManager: () {
                    Navigator.of(sheetCtx).pop();
                    onOpenDeviceManager();
                  },
                  showFooterButton: true,
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _DeviceListContent extends StatelessWidget {
  const _DeviceListContent({
    required this.peers,
    required this.activeId,
    required this.onSelect,
    required this.onOpenManager,
    required this.showFooterButton,
  });

  final List<PairedPeerModel> peers;
  final String? activeId;
  final ValueChanged<PairedPeerModel> onSelect;
  final VoidCallback onOpenManager;
  final bool showFooterButton;

  IconData _iconForOs(String osType) {
    final lower = osType.toLowerCase();
    if (lower.contains('android')) return Icons.phone_android_rounded;
    if (lower.contains('win')) return Icons.desktop_windows_rounded;
    return Icons.laptop_mac_rounded;
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: <Widget>[
        if (peers.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 16),
            child: Text(
              '尚未配对任何设备，点击下方按钮即可扫码或局域网发现配对。',
              style: Theme.of(context).textTheme.bodySmall,
            ),
          )
        else
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 280),
            child: ListView.separated(
              shrinkWrap: true,
              itemCount: peers.length,
              separatorBuilder: (_, __) => const SizedBox(height: 8),
              itemBuilder: (context, index) {
                final peer = peers[index];
                final isSelected = peer.deviceId == activeId;
                final addr = peer.ip == null
                    ? peer.osType
                    : '${peer.ip}${peer.port != null ? ':${peer.port}' : ''}';
                return InkWell(
                  onTap: () => onSelect(peer),
                  borderRadius: const BorderRadius.all(Radius.circular(14)),
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 11,
                    ),
                    decoration: BoxDecoration(
                      color: isSelected
                          ? neu.accentSoft
                          : neu.insetSurface,
                      borderRadius: const BorderRadius.all(Radius.circular(14)),
                      border: Border.all(
                        color: isSelected
                            ? neu.accent.withValues(alpha: 0.4)
                            : neu.hairlineBorder,
                      ),
                    ),
                    child: Row(
                      children: <Widget>[
                        NeuIconBadge(
                          icon: _iconForOs(peer.osType),
                          color: isSelected
                              ? neu.accent
                              : neu.slateMuted,
                          backgroundColor: neu.surface,
                          size: 36,
                          iconSize: 18,
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                peer.deviceName,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: Theme.of(context)
                                    .textTheme
                                    .titleSmall
                                    ?.copyWith(fontWeight: FontWeight.w600),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                addr,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: Theme.of(context).textTheme.bodySmall,
                              ),
                            ],
                          ),
                        ),
                        const SizedBox(width: 8),
                        NeuStatusBadge(
                          label: peer.online ? '在线' : '离线',
                          tone: peer.online
                              ? NeuStatusTone.healthy
                              : NeuStatusTone.neutral,
                          showDot: true,
                        ),
                        if (isSelected) ...<Widget>[
                          const SizedBox(width: 8),
                          Icon(
                            Icons.check_rounded,
                            size: 18,
                            color: neu.accent,
                          ),
                        ],
                      ],
                    ),
                  ),
                );
              },
            ),
          ),
        if (showFooterButton) ...<Widget>[
          const SizedBox(height: 14),
          NeuButton(
            label: '管理全部设备 / 配对新设备',
            icon: Icons.add_link_rounded,
            tone: NeuButtonTone.primary,
            expanded: true,
            onPressed: onOpenManager,
          ),
        ],
      ],
    );
  }
}
