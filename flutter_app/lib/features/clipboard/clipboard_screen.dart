// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 4 / 6 节 (push_clipboard_text)
// - lanvault_complete_blueprint.md 第 3.3 节 (复用 E2EE 加密信道的局域网剪贴板双向快传)

import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../theme/neu_theme.dart';

class _ClipboardEntry {
  const _ClipboardEntry({
    required this.targetLabel,
    required this.text,
    required this.timestamp,
  });

  final String targetLabel;
  final String text;
  final DateTime timestamp;
}

/// 局域网加密剪贴板双向快传页面。
class ClipboardScreen extends StatefulWidget {
  const ClipboardScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<ClipboardScreen> createState() => _ClipboardScreenState();
}

class _ClipboardScreenState extends State<ClipboardScreen> {
  final TextEditingController _targetDeviceController = TextEditingController();
  final TextEditingController _contentController = TextEditingController();
  final List<_ClipboardEntry> _history = <_ClipboardEntry>[];

  bool _sending = false;
  String? _feedback;
  String? _error;

  @override
  void dispose() {
    _targetDeviceController.dispose();
    _contentController.dispose();
    super.dispose();
  }

  Future<void> _pasteFromSystemClipboard() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    if (!mounted) return;
    final text = data?.text;
    if (text == null || text.isEmpty) {
      setState(() => _error = '系统剪贴板当前没有纯文本内容。');
      return;
    }
    setState(() {
      _contentController.text = text;
      _error = null;
    });
  }

  Future<void> _pushClipboard() async {
    final text = _contentController.text;
    if (text.isEmpty) {
      setState(() => _error = '请先输入或粘贴要发送的文本内容。');
      return;
    }
    final rawTarget = _targetDeviceController.text.trim();
    final targetId = rawTarget.isEmpty ? null : rawTarget;

    setState(() {
      _sending = true;
      _error = null;
      _feedback = null;
    });

    try {
      await widget.bridge.pushClipboardText(
        targetDeviceId: targetId,
        text: text,
      );
      if (!mounted) return;
      setState(() {
        _sending = false;
        _history.insert(
          0,
          _ClipboardEntry(
            targetLabel: targetId ?? '全部在线已配对设备',
            text: text,
            timestamp: DateTime.now(),
          ),
        );
        _feedback = '已通过 XChaCha20-Poly1305 信道推送至 ${targetId ?? '全部在线设备'}。';
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _sending = false;
        _error = '剪贴板推送失败：$err';
      });
    }
  }

  Future<void> _copyToSystemClipboard(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (!mounted) return;
    setState(() => _feedback = '已复制到本机系统剪贴板。');
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;

    return SingleChildScrollView(
      padding: const EdgeInsets.all(NeuTokens.spaceLg),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text('局域网加密剪贴板', style: textTheme.headlineSmall),
          const SizedBox(height: NeuTokens.spaceXs),
          Text(
            '复用已配对设备间的加密信道进行文本双向快传，不经外网中转',
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
                Text('发送加密文本', style: textTheme.titleMedium),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuInsetField(
                  controller: _targetDeviceController,
                  label: '目标设备 ID（留空则广播至全部在线已配对设备）',
                  hint: '例如：peer-win-desktop',
                  prefixIcon: Icons.devices_outlined,
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                NeuInsetField(
                  controller: _contentController,
                  label: '剪贴板内容',
                  hint: '输入或从系统剪贴板读取文本…',
                  maxLines: 5,
                ),
                const SizedBox(height: NeuTokens.spaceMd),
                Wrap(
                  spacing: NeuTokens.spaceSm,
                  runSpacing: NeuTokens.spaceSm,
                  children: <Widget>[
                    NeuButton(
                      label: '加密发送',
                      icon: Icons.send_outlined,
                      tone: NeuButtonTone.primary,
                      loading: _sending,
                      onPressed: _pushClipboard,
                    ),
                    NeuButton(
                      label: '读取本机剪贴板',
                      icon: Icons.content_paste_go,
                      onPressed: _pasteFromSystemClipboard,
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
                Text('本次会话快传记录', style: textTheme.titleMedium),
                const SizedBox(height: NeuTokens.spaceMd),
                if (_history.isEmpty)
                  NeuCard(
                    inset: true,
                    child: Text('暂无快传记录。', style: textTheme.bodySmall),
                  )
                else
                  for (final item in _history) ...<Widget>[
                    NeuCard(
                      inset: true,
                      padding: const EdgeInsets.all(12),
                      child: Row(
                        children: <Widget>[
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: <Widget>[
                                Text(
                                  '${item.targetLabel} · '
                                  '${item.timestamp.hour.toString().padLeft(2, '0')}:'
                                  '${item.timestamp.minute.toString().padLeft(2, '0')}:'
                                  '${item.timestamp.second.toString().padLeft(2, '0')}',
                                  style: textTheme.labelMedium,
                                ),
                                const SizedBox(height: 4),
                                Text(
                                  item.text,
                                  maxLines: 3,
                                  overflow: TextOverflow.ellipsis,
                                  style: textTheme.bodyMedium,
                                ),
                              ],
                            ),
                          ),
                          const SizedBox(width: NeuTokens.spaceSm),
                          NeuButton(
                            label: '复制',
                            icon: Icons.copy,
                            onPressed: () => _copyToSystemClipboard(item.text),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: NeuTokens.spaceSm),
                  ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}
