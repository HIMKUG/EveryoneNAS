// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 4 / 6 节 (start_mdns_scan, pair_with_pin, enable_auto_reconnect)
// - lanvault_complete_blueprint.md 第 3.3 节 (mDNS 局域网发现 + 6 位 PIN 配对 + 指数退避自动重连)

import 'dart:async';

import 'package:flutter/material.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';

/// 局域网扫描与零信任配对页面。
class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final Map<String, DiscoveredPeer> _peers = <String, DiscoveredPeer>{};
  final TextEditingController _pinController = TextEditingController();
  final TextEditingController _ipController = TextEditingController();
  final TextEditingController _portController = TextEditingController(text: '7842');

  StreamSubscription<DiscoveredPeer>? _scanSub;
  bool _scanning = false;
  bool _pairing = false;
  bool _autoReconnect = true;
  String? _selectedDeviceId;
  String? _feedback;
  String? _error;

  @override
  void dispose() {
    _scanSub?.cancel();
    _pinController.dispose();
    _ipController.dispose();
    _portController.dispose();
    super.dispose();
  }

  void _toggleScan() {
    if (_scanning) {
      _scanSub?.cancel();
      _scanSub = null;
      setState(() => _scanning = false);
      return;
    }

    setState(() {
      _scanning = true;
      _error = null;
      _feedback = '正在通过 mDNS (_lanvault._tcp.local.) 扫描局域网设备…';
    });

    try {
      _scanSub = widget.bridge.startMdnsScan().listen(
        (peer) {
          if (!mounted) return;
          setState(() {
            _peers[peer.deviceId] = peer;
          });
        },
        onError: (Object err) {
          _scanSub?.cancel();
          _scanSub = null;
          if (!mounted) return;
          setState(() {
            _scanning = false;
            _error = 'mDNS 扫描失败：$err';
          });
        },
        onDone: () {
          _scanSub = null;
          if (!mounted) return;
          setState(() => _scanning = false);
        },
        cancelOnError: true,
      );
    } catch (err) {
      setState(() {
        _scanning = false;
        _error = '启动扫描失败：$err';
      });
    }
  }

  void _selectPeer(DiscoveredPeer peer) {
    setState(() {
      _selectedDeviceId = peer.deviceId;
      _ipController.text = peer.ip;
      _portController.text = peer.port.toString();
      _error = null;
    });
  }

  Future<void> _submitPair() async {
    final ip = _ipController.text.trim();
    final port = int.tryParse(_portController.text.trim());
    final pin = _pinController.text.trim();

    if (ip.isEmpty || port == null || port <= 0 || port > 65535) {
      setState(() => _error = '请输入有效的设备 IP 与端口（1–65535）。');
      return;
    }
    if (!RegExp(r'^\d{6}$').hasMatch(pin)) {
      setState(() => _error = 'PIN 码必须为 6 位数字。');
      return;
    }

    setState(() {
      _pairing = true;
      _error = null;
      _feedback = null;
    });

    try {
      final pairedDeviceId = await widget.bridge.pairWithPin(
        ip: ip,
        port: port,
        pin: pin,
      );
      if (!mounted) return;
      _pinController.clear();
      setState(() {
        _pairing = false;
        _feedback = '配对成功，已建立 Ed25519 信任绑定（设备 ID：$pairedDeviceId）。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _pairing = false;
        _error = 'PIN 配对失败：$err';
      });
    }
  }

  Future<void> _toggleAutoReconnect(bool enabled) async {
    setState(() {
      _autoReconnect = enabled;
      _error = null;
    });
    try {
      await widget.bridge.enableAutoReconnect(enabled);
      if (!mounted) return;
      setState(() {
        _feedback = enabled ? '已开启局域网断线指数退避自动重连。' : '已关闭自动重连。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _autoReconnect = !enabled;
        _error = '更新自动重连策略失败：$err';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final peerList = _peers.values.toList(growable: false);
    final innerRadius = NeuRadius.concentric(NeuRadius.card, NeuSpace.md);

    return SingleChildScrollView(
      padding: const EdgeInsets.all(NeuSpace.lg),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text('设备发现与配对', style: textTheme.headlineSmall),
          const SizedBox(height: NeuSpace.xs),
          Text(
            '扫描局域网 EveryoneNAS 节点并通过 6 位动态 PIN 建立双向加密信任',
            style: textTheme.bodySmall,
          ),
          const SizedBox(height: NeuSpace.lg),
          if (_error != null) ...<Widget>[
            NeuCard(
              inset: true,
              child: Text(
                _error!,
                style: textTheme.bodyMedium?.copyWith(color: NeuTokens.danger),
              ),
            ),
            const SizedBox(height: NeuSpace.md),
          ],
          if (_feedback != null) ...<Widget>[
            NeuCard(
              inset: true,
              child: Text(
                _feedback!,
                style: textTheme.bodyMedium?.copyWith(color: NeuTokens.healthy),
              ),
            ),
            const SizedBox(height: NeuSpace.md),
          ],
          NeuCard(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: <Widget>[
                    Text('局域网 mDNS 扫描列表', style: textTheme.titleMedium),
                    NeuButton(
                      label: _scanning ? '停止扫描' : '开始扫描',
                      icon: _scanning ? Icons.stop : Icons.radar,
                      tone: _scanning
                          ? NeuButtonTone.danger
                          : NeuButtonTone.primary,
                      onPressed: _toggleScan,
                    ),
                  ],
                ),
                const SizedBox(height: NeuSpace.md),
                if (peerList.isEmpty)
                  NeuCard(
                    inset: true,
                    radius: innerRadius,
                    child: Text(
                      _scanning
                          ? '正在等待局域网节点广播…'
                          : '点击“开始扫描”发现局域网内在线的 Windows / Android 节点。',
                      style: textTheme.bodySmall,
                    ),
                  )
                else
                  for (final peer in peerList) ...<Widget>[
                    _PeerTile(
                      peer: peer,
                      selected: peer.deviceId == _selectedDeviceId,
                      onTap: () => _selectPeer(peer),
                    ),
                    const SizedBox(height: NeuSpace.sm),
                  ],
              ],
            ),
          ),
          const SizedBox(height: NeuSpace.md),
          NeuCard(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text('6 位 PIN 配对', style: textTheme.titleMedium),
                const SizedBox(height: NeuSpace.md),
                Row(
                  children: <Widget>[
                    Expanded(
                      flex: 3,
                      child: NeuInsetField(
                        controller: _ipController,
                        label: '目标局域网 IP',
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
                const SizedBox(height: NeuSpace.md),
                NeuInsetField(
                  controller: _pinController,
                  label: '6 位动态 PIN',
                  hint: '输入对端显示的 6 位数字',
                  keyboardType: TextInputType.number,
                  prefixIcon: Icons.pin_outlined,
                  onSubmitted: (_) => _submitPair(),
                ),
                const SizedBox(height: NeuSpace.md),
                NeuButton(
                  label: '验证 PIN 并绑定信任',
                  icon: Icons.verified_user_outlined,
                  tone: NeuButtonTone.primary,
                  loading: _pairing,
                  expanded: true,
                  onPressed: _submitPair,
                ),
              ],
            ),
          ),
          const SizedBox(height: NeuSpace.md),
          NeuCard(
            child: NeuSwitch(
              title: '断线自动重连',
              subtitle: '网络恢复后按 2s / 5s / 15s / 30s 指数退避探测已配对节点',
              value: _autoReconnect,
              onChanged: _toggleAutoReconnect,
            ),
          ),
        ],
      ),
    );
  }
}

class _PeerTile extends StatelessWidget {
  const _PeerTile({
    required this.peer,
    required this.selected,
    required this.onTap,
  });

  final DiscoveredPeer peer;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final minTouch = NeuTokens.resolveMinTouchTarget(context);
    final tileRadius = NeuRadius.concentric(NeuRadius.card, NeuSpace.sm);

    return RepaintBoundary(
      child: Semantics(
        button: true,
        selected: selected,
        label: '${peer.deviceName} ${peer.ip}:${peer.port}',
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: onTap,
          child: AnimatedContainer(
            duration: NeuMotion.resolvePress(context),
            curve: NeuMotion.resolvePressCurve(context),
            constraints: BoxConstraints(minHeight: minTouch),
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              color: selected ? NeuTokens.insetSurface : NeuTokens.surface,
              borderRadius: BorderRadius.all(Radius.circular(tileRadius)),
              border: Border.all(
                color: selected ? NeuTokens.accent : NeuTokens.hairlineBorder,
                width: selected ? 1.5 : 1.0,
              ),
            ),
            child: Row(
              children: <Widget>[
                Icon(
                  peer.osType.toLowerCase() == 'android'
                      ? Icons.phone_android
                      : Icons.desktop_windows,
                  color: NeuTokens.textPrimary,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(peer.deviceName, style: textTheme.titleSmall),
                      Text(
                        '${peer.ip}:${peer.port} · 指纹 ${peer.fingerprint}',
                        style: textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                Text(
                  selected ? '已选择' : '选择',
                  style: textTheme.labelMedium?.copyWith(color: NeuTokens.accent),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
