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
class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final Map<String, DiscoveredPeer> _discovered = <String, DiscoveredPeer>{};
  StreamSubscription<DiscoveredPeer>? _scanSub;
  Timer? _refreshTimer;
  Timer? _scanTimeoutTimer;

  _DeviceTab _activeTab = _DeviceTab.paired;
  bool _scanning = false;
  bool _loadingPaired = false;

  @override
  void initState() {
    super.initState();
    _refreshPairedPeers();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // 定期刷新已配对列表，让"在线/离线"随握手租约（45s）及时收敛，
    // 用户不需要切页或重启就能看到两端状态一致。
    _refreshTimer ??= Timer.periodic(
      const Duration(seconds: 5),
      (_) => unawaited(_refreshPairedPeers(silent: true)),
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
        NeuToast.error(context, '读取已配对设备失败：${LanVaultBridge.formatUserError(err)}');
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
          NeuToast.error(context, '扫描失败：${LanVaultBridge.formatUserError(err)}');
        },
        onDone: () {
          _stopScan(silent: true);
        },
        cancelOnError: true,
      );
    } catch (err) {
      _stopScan(silent: true);
      if (mounted) {
        NeuToast.error(context, '启动扫描失败：${LanVaultBridge.formatUserError(err)}');
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
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
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
                            '管理已配对的设备与局域网发现',
                            style: textTheme.bodySmall,
                          ),
                        ],
                      ),
                    )
                  else
                    Expanded(
                      child: Text(
                        '管理已配对设备与局域网发现',
                        style: textTheme.bodySmall,
                      ),
                    ),
                  const SizedBox(width: NeuSpace.sm),
                  NeuButton(
                    label: '配对新设备',
                    icon: Icons.add_rounded,
                    tone: NeuButtonTone.primary,
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
            ],
          ),
        );
      },
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
                  padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 4),
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
                        itemBuilder: (context) => const <PopupMenuEntry<String>>[
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
                        onPressed: () => _openPairDialog(initialIp: peer.ip, initialPort: peer.port),
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
      NeuToast.error(context, '读取本机配对信息失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _submitInteractiveRequest(String address, int port) async {
    setState(() => _pairing = true);
    try {
      NeuToast.info(context, '请在对方设备确认配对');
      final pairedId = await widget.bridge.requestPairing(ip: address, port: port);
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
    final localFingerprint = Uri.tryParse(info?.pairingQrUri ?? '')?.queryParameters['fp'];

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
                      await Clipboard.setData(ClipboardData(text: localFingerprint));
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
