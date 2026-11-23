import 'dart:io';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

enum _TransferMode { file, clipboard }

/// 6. 传输页：
/// - 顶部两张大动作卡：“发送文件（调用 push_clipboard_file）”与“剪贴板同步（调用 push_clipboard_text）”
/// - 下方目标设备选择、文件/文本输入与最近传输列表（带文件/文本图标、目标设备、时间戳与绿色完成勾）
class ClipboardScreen extends StatefulWidget {
  const ClipboardScreen({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<ClipboardScreen> createState() => _ClipboardScreenState();
}

class _ClipboardScreenState extends State<ClipboardScreen> {
  static const String _broadcastTargetSentinel = '__all_paired__';

  final TextEditingController _filePathController = TextEditingController();
  final TextEditingController _contentController = TextEditingController();
  List<TransferHistoryModel> _history = const <TransferHistoryModel>[];

  _TransferMode _mode = _TransferMode.file;
  String? _selectedTarget;
  bool _sending = false;

  @override
  void initState() {
    super.initState();
    _refreshPeers();
    _loadHistory();
  }

  Future<void> _loadHistory() async {
    try {
      final records = await widget.bridge.listTransferHistory(limit: 50);
      if (!mounted) return;
      setState(() => _history = records);
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '加载传输记录失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  @override
  void dispose() {
    _filePathController.dispose();
    _contentController.dispose();
    super.dispose();
  }

  String? _resolveValidTarget(List<PairedPeerModel> peers) {
    if (peers.isEmpty) return null;
    if (_selectedTarget == _broadcastTargetSentinel && peers.length > 1) {
      return _broadcastTargetSentinel;
    }
    if (_selectedTarget != null &&
        peers.any((p) => p.deviceId == _selectedTarget)) {
      return _selectedTarget;
    }
    return peers.length > 1 ? _broadcastTargetSentinel : peers.first.deviceId;
  }

  Future<void> _refreshPeers() async {
    try {
      final peers = await widget.bridge.refreshPairedPeers();
      if (!mounted) return;
      final resolved = _resolveValidTarget(peers);
      if (resolved != _selectedTarget) {
        setState(() => _selectedTarget = resolved);
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '获取已配对设备失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _pickFileToSend() async {
    setState(() => _mode = _TransferMode.file);
    try {
      final picked = await widget.bridge.pickFile(title: '选择要加密传输的文件');
      if (picked != null && mounted) {
        setState(() => _filePathController.text = picked);
      } else if (!Platform.isWindows && mounted) {
        NeuToast.info(context, '请在下方输入或粘贴本地文件路径');
      }
    } catch (err) {
      if (!mounted) return;
      NeuToast.error(context, '打开文件选择器失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _pasteFromSystemClipboard() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text;
    if (!mounted) return;
    if (text == null || text.isEmpty) {
      NeuToast.info(context, '系统剪贴板当前为空');
      return;
    }
    setState(() {
      _mode = _TransferMode.clipboard;
      _contentController.text = text;
    });
  }

  Future<void> _sendFile(
    List<PairedPeerModel> peers,
    String? effectiveSelection,
  ) async {
    if (peers.isEmpty) {
      NeuToast.warning(context, '请先在“设备”页配对至少一台设备');
      return;
    }
    final filePath = _filePathController.text.trim();
    if (filePath.isEmpty) {
      NeuToast.warning(context, '请先选择或输入要发送的文件路径');
      return;
    }

    final targetId = (effectiveSelection == null ||
            effectiveSelection == _broadcastTargetSentinel)
        ? null
        : effectiveSelection;

    setState(() {
      _selectedTarget = effectiveSelection;
      _sending = true;
    });
    try {
      final sent = await widget.bridge.pushClipboardFile(
        targetDeviceId: targetId,
        filePath: filePath,
      );
      if (!mounted) return;
      setState(() => _sending = false);
      if (!sent) {
        NeuToast.warning(context, '文件未发送成功，请确认对端设备在线');
        return;
      }

      final fileName = SystemEnvHelper.folderDisplayName(filePath);
      _filePathController.clear();
      await _loadHistory();
      if (!mounted) return;
      NeuToast.success(context, '已加密发送文件：$fileName');
    } catch (err) {
      if (!mounted) return;
      setState(() => _sending = false);
      NeuToast.error(context, '发送文件失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _sendClipboardText(
    List<PairedPeerModel> peers,
    String? effectiveSelection,
  ) async {
    if (peers.isEmpty) {
      NeuToast.warning(context, '请先在“设备”页配对至少一台设备');
      return;
    }

    final text = _contentController.text.trim();
    if (text.isEmpty) {
      NeuToast.warning(context, '请输入要同步的剪贴板内容');
      return;
    }

    final targetId = (effectiveSelection == null ||
            effectiveSelection == _broadcastTargetSentinel)
        ? null
        : effectiveSelection;

    setState(() {
      _selectedTarget = effectiveSelection;
      _sending = true;
    });
    try {
      await widget.bridge.pushClipboardText(
        targetDeviceId: targetId,
        text: text,
      );
      if (!mounted) return;
      _contentController.clear();
      setState(() => _sending = false);
      await _loadHistory();
      if (!mounted) return;
      NeuToast.success(context, '已同步剪贴板内容');
    } catch (err) {
      if (!mounted) return;
      setState(() => _sending = false);
      NeuToast.error(context, '同步剪贴板失败：${LanVaultBridge.formatUserError(err)}');
    }
  }

  Future<void> _copyMessage(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (!mounted) return;
    NeuToast.success(context, '已复制到剪贴板');
  }

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return ValueListenableBuilder<List<PairedPeerModel>>(
      valueListenable: widget.bridge.pairedPeersNotifier,
      builder: (context, peers, _) {
        final options = peers.isEmpty
            ? const <NeuDropdownOption<String>>[]
            : <NeuDropdownOption<String>>[
                if (peers.length > 1)
                  const NeuDropdownOption<String>(
                    value: _broadcastTargetSentinel,
                    label: '全部在线已配对设备',
                  ),
                for (final peer in peers)
                  NeuDropdownOption<String>(
                    value: peer.deviceId,
                    label: peer.displayLabel,
                  ),
              ];

        final validSelection = _resolveValidTarget(peers);

        return SingleChildScrollView(
          padding: EdgeInsets.all(isWide ? NeuSpace.lg : NeuSpace.md),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              if (isWide) ...<Widget>[
                Text('传输', style: textTheme.headlineSmall),
                const SizedBox(height: 4),
                Text(
                  '局域网内的加密文件传输与协作',
                  style: textTheme.bodySmall,
                ),
                const SizedBox(height: 18),
              ],
              Row(
                children: <Widget>[
                  Expanded(
                    child: _TransferActionCard(
                      icon: Icons.send_rounded,
                      title: '发送文件',
                      subtitle: '加密传输至设备',
                      selected: _mode == _TransferMode.file,
                      onTap: _pickFileToSend,
                    ),
                  ),
                  const SizedBox(width: NeuSpace.md),
                  Expanded(
                    child: _TransferActionCard(
                      icon: Icons.content_paste_go_rounded,
                      title: '剪贴板同步',
                      subtitle: '跨设备内容同步',
                      selected: _mode == _TransferMode.clipboard,
                      onTap: () =>
                          setState(() => _mode = _TransferMode.clipboard),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: NeuSpace.md),
              NeuCard(
                padding: const EdgeInsets.all(20),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    NeuDropdown<String>(
                      label: '目标设备',
                      emptyHint: '暂无已配对设备（请先在“设备”页配对）',
                      prefixIcon: Icons.devices_outlined,
                      options: options,
                      value: validSelection,
                      enabled: peers.isNotEmpty,
                      onChanged: (value) =>
                          setState(() => _selectedTarget = value),
                    ),
                    const SizedBox(height: 14),
                    NeuMotionSwitcher(
                      child: _mode == _TransferMode.file
                          ? Column(
                              key: const ValueKey<_TransferMode>(_TransferMode.file),
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: <Widget>[
                                if (isWide)
                                  Row(
                                    crossAxisAlignment: CrossAxisAlignment.end,
                                    children: <Widget>[
                                      Expanded(
                                        child: NeuInsetField(
                                          controller: _filePathController,
                                          label: '本地文件路径',
                                          hint: Platform.isAndroid
                                              ? '/storage/emulated/0/Download/report.pdf'
                                              : r'C:\Users\YourName\Desktop\report.pdf',
                                          prefixIcon:
                                              Icons.insert_drive_file_outlined,
                                        ),
                                      ),
                                      const SizedBox(width: NeuSpace.sm),
                                      NeuButton(
                                        label: '选择文件',
                                        icon: Icons.attach_file_rounded,
                                        onPressed: _pickFileToSend,
                                      ),
                                    ],
                                  )
                                else ...<Widget>[
                                  NeuInsetField(
                                    controller: _filePathController,
                                    label: '本地文件路径',
                                    hint: Platform.isAndroid
                                        ? '/storage/emulated/0/Download/report.pdf'
                                        : r'C:\Users\YourName\Desktop\report.pdf',
                                    prefixIcon:
                                        Icons.insert_drive_file_outlined,
                                  ),
                                  const SizedBox(height: NeuSpace.sm),
                                  NeuButton(
                                    label: '选择文件',
                                    icon: Icons.attach_file_rounded,
                                    expanded: true,
                                    onPressed: _pickFileToSend,
                                  ),
                                ],
                                const SizedBox(height: 14),
                                Align(
                                  alignment: isWide
                                      ? Alignment.centerLeft
                                      : Alignment.center,
                                  child: NeuButton(
                                    label: '加密发送文件',
                                    icon: Icons.send_rounded,
                                    tone: NeuButtonTone.primary,
                                    loading: _sending,
                                    expanded: !isWide,
                                    onPressed: peers.isEmpty
                                        ? null
                                        : () => _sendFile(peers, validSelection),
                                  ),
                                ),
                              ],
                            )
                          : Column(
                              key: const ValueKey<_TransferMode>(
                                _TransferMode.clipboard,
                              ),
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: <Widget>[
                                NeuInsetField(
                                  controller: _contentController,
                                  label: '剪贴板内容',
                                  hint: '输入或粘贴要跨设备同步的文本…',
                                  maxLines: 4,
                                ),
                                const SizedBox(height: 14),
                                Wrap(
                                  spacing: NeuSpace.sm,
                                  runSpacing: NeuSpace.sm,
                                  children: <Widget>[
                                    NeuButton(
                                      label: '加密同步剪贴板',
                                      icon: Icons.sync_alt_rounded,
                                      tone: NeuButtonTone.primary,
                                      loading: _sending,
                                      onPressed: peers.isEmpty
                                          ? null
                                          : () => _sendClipboardText(
                                                peers,
                                                validSelection,
                                              ),
                                    ),
                                    NeuButton(
                                      label: '读取本机剪贴板',
                                      icon: Icons.content_paste_rounded,
                                      onPressed: _pasteFromSystemClipboard,
                                    ),
                                  ],
                                ),
                              ],
                            ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: NeuSpace.md),
              NeuCard(
                padding: const EdgeInsets.all(20),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: <Widget>[
                        Text('最近传输', style: textTheme.titleMedium),
                        Text(
                          '${_history.length} 条记录',
                          style: textTheme.labelMedium,
                        ),
                      ],
                    ),
                    const SizedBox(height: 14),
                    if (_history.isEmpty)
                      const NeuEmptyState(
                        icon: Icons.swap_horiz_rounded,
                        title: '暂无传输记录',
                        subtitle: '选择上方“发送文件”或“剪贴板同步”即可向局域网设备高速加密发送。',
                      )
                    else
                      for (var i = 0; i < _history.length; i++) ...<Widget>[
                        Builder(
                          key: ValueKey<String>(
                            '${_history[i].formattedTime}_${_history[i].title}_$i',
                          ),
                          builder: (context) {
                            final item = _history[i];
                            return Padding(
                              padding: const EdgeInsets.symmetric(vertical: 10),
                              child: Row(
                                children: <Widget>[
                                  NeuIconBadge(
                                    icon: item.isFile
                                        ? Icons.description_rounded
                                        : Icons.notes_rounded,
                                    color: item.isFile
                                        ? NeuTokens.folderGold
                                        : NeuTokens.accent,
                                    backgroundColor: item.isFile
                                        ? NeuTokens.folderGoldSoft
                                        : NeuTokens.accentSoft,
                                    radius: 10,
                                  ),
                                  const SizedBox(width: 12),
                                  Expanded(
                                    child: Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: <Widget>[
                                        Text(
                                          item.title,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: textTheme.titleSmall,
                                        ),
                                        const SizedBox(height: 2),
                                        Text(
                                          isWide
                                              ? item.subtitle
                                              : '${item.subtitle} · ${item.formattedTime}',
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: textTheme.bodySmall,
                                        ),
                                      ],
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  if (item.content != null &&
                                      item.content!.isNotEmpty)
                                    IconButton(
                                      tooltip: '复制文本',
                                      icon: const Icon(
                                        Icons.copy_rounded,
                                        size: 18,
                                        color: NeuTokens.textSecondary,
                                      ),
                                      onPressed: () =>
                                          _copyMessage(item.content!),
                                    ),
                                  if (isWide) ...<Widget>[
                                    Text(
                                      item.formattedTime,
                                      style: textTheme.labelMedium,
                                    ),
                                    const SizedBox(width: 8),
                                  ],
                                  const Icon(
                                    Icons.check_circle_outline_rounded,
                                    size: 18,
                                    color: NeuTokens.healthy,
                                  ),
                                ],
                              ),
                            );
                          },
                        ),
                        if (i < _history.length - 1) const Divider(height: 1),
                      ],
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

class _TransferActionCard extends StatelessWidget {
  const _TransferActionCard({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.selected,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      onTap: onTap,
      backgroundColor: selected ? NeuTokens.accentSoft : NeuTokens.surface,
      borderColor: selected
          ? NeuTokens.accent.withValues(alpha: 0.45)
          : NeuTokens.hairlineBorder,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 24),
      child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: <Widget>[
              Container(
                width: 48,
                height: 48,
                decoration: BoxDecoration(
                  color: selected ? NeuTokens.surface : NeuTokens.accentSoft,
                  borderRadius: const BorderRadius.all(Radius.circular(14)),
                ),
                child: Icon(icon, size: 24, color: NeuTokens.accent),
              ),
              const SizedBox(height: 12),
              Text(
                title,
                style: textTheme.titleMedium?.copyWith(
                  color: selected ? NeuTokens.accent : NeuTokens.textPrimary,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                subtitle,
                textAlign: TextAlign.center,
                style: textTheme.bodySmall,
              ),
            ],
          ),
        );
  }
}
