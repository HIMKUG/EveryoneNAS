import 'dart:async';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../../bridge/lanvault_bridge.dart';
import '../../bridge/system_env_helper.dart';
import '../../theme/neu_theme.dart';
import '../../theme/neu_toast.dart';

/// 传输页（对话流重构版，对标 Message 体验）：
/// - 彻底合并“发送文件”与“剪贴板同步”为统一的聊天式传输流。
/// - 桌面端（>= 840px）：左栏已配对设备列表 + 右栏消息与文件气泡流 + 底部输入栏。
/// - 移动端（< 840px）：跟随顶栏全局设备胶囊，全屏呈现对话流 + 底部输入栏。
class ClipboardScreen extends StatefulWidget {
  const ClipboardScreen({
    super.key,
    required this.bridge,
    this.onOpenDeviceManager,
  });

  final LanVaultBridge bridge;
  final VoidCallback? onOpenDeviceManager;

  @override
  State<ClipboardScreen> createState() => _ClipboardScreenState();
}

class _ClipboardScreenState extends State<ClipboardScreen> {
  final TextEditingController _inputController = TextEditingController();
  final ScrollController _scrollController = ScrollController();
  List<TransferHistoryModel> _history = const <TransferHistoryModel>[];
  Timer? _pollTimer;
  final _outbox = <String, OutboxMessageViewModel>{};
  final _pending = <int, _PendingChatMessage>{};
  int _nextPendingId = -1;
  bool _loading = true;
  bool _refreshing = false;

  @override
  void initState() {
    super.initState();
    unawaited(_refreshHistory(silent: false));
    _pollTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => unawaited(_refreshHistory(silent: true)),
    );
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _inputController.dispose();
    _scrollController.dispose();
    super.dispose();
  }

  Future<void> _refreshHistory({required bool silent}) async {
    if (_refreshing) return;
    _refreshing = true;
    if (!silent && mounted) setState(() => _loading = true);
    try {
      final list = await widget.bridge.listTransferHistory(limit: 100);
      final outgoing = await widget.bridge.listOutboxMessages(limit: 100);
      if (!mounted) return;
      setState(() {
        _history = list.reversed.toList(growable: false);
        _outbox.addEntries(
            outgoing.map((item) => MapEntry(item.clientMsgId, item)));
        _loading = false;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() => _loading = false);
      if (!silent) {
        NeuToast.error(context, LanVaultBridge.formatUserError(err));
      }
    } finally {
      _refreshing = false;
    }
  }

  Future<void> _sendText() async {
    final text = _inputController.text.trim();
    final peer = widget.bridge.selectedPeer;
    if (text.isEmpty || peer == null) return;
    _inputController.clear();
    await _enqueue(
      peer.deviceId,
      title: text,
      kind: 'clipboard',
      submit: () => widget.bridge.enqueueMessageText(
        targetDeviceIds: [peer.deviceId],
        text: text,
      ),
    );
  }

  Future<void> _pickAndSendFile() async {
    final peer = widget.bridge.selectedPeer;
    if (peer == null) return;
    final path = await widget.bridge.pickFile(title: '选择要发送的文件');
    if (!mounted || path == null || path.isEmpty) return;
    await _enqueue(
      peer.deviceId,
      title: SystemEnvHelper.folderDisplayName(path),
      kind: 'file',
      submit: () => widget.bridge.enqueueMessageFile(
        targetDeviceIds: [peer.deviceId],
        filePath: path,
      ),
    );
  }

  Future<void> _enqueue(
    String deviceId, {
    required String title,
    required String kind,
    required Future<OutboxMessageViewModel> Function() submit,
  }) async {
    final id = _nextPendingId--;
    final item = TransferHistoryModel(
      id: id,
      direction: 'sent',
      kind: kind,
      title: title,
      subtitle: '',
      peerName: deviceId,
      sizeBytes: 0,
      createdAtSecs: DateTime.now().millisecondsSinceEpoch ~/ 1000,
    );
    setState(() => _pending[id] = (item: item, error: null, retry: null));
    _scrollToBottom();

    Future<void> deliver() async {
      try {
        final queued = await submit();
        if (!mounted) return;
        setState(() {
          _pending.remove(id);
          _outbox[queued.clientMsgId] = queued;
        });
      } catch (err) {
        if (!mounted) return;
        final error = LanVaultBridge.formatUserError(err);
        setState(() => _pending[id] = (
              item: item,
              error: error,
              retry: () async {
                setState(() =>
                    _pending[id] = (item: item, error: null, retry: null));
                await deliver();
              },
            ));
      }
    }

    await deliver();
  }

  Future<void> _retryOutbox(String id) async {
    try {
      await widget.bridge.retryOutboxMessage(id);
      await _refreshHistory(silent: true);
    } catch (err) {
      if (mounted) NeuToast.error(context, LanVaultBridge.formatUserError(err));
    }
  }

  Future<void> _pasteFromClipboard() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim() ?? '';
    if (!mounted) return;
    if (text.isEmpty) {
      NeuToast.info(context, '本机剪贴板为空');
      return;
    }
    _inputController.text = text;
    _inputController.selection = TextSelection.fromPosition(
      TextPosition(offset: text.length),
    );
  }

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scrollController.hasClients) {
        _scrollController.animateTo(
          _scrollController.position.maxScrollExtent,
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
        );
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return ValueListenableBuilder<List<PairedPeerModel>>(
      valueListenable: widget.bridge.pairedPeersNotifier,
      builder: (context, peers, _) {
        return ValueListenableBuilder<String?>(
          valueListenable: widget.bridge.selectedPeerIdNotifier,
          builder: (context, _, __) {
            final activePeer = widget.bridge.selectedPeer;
            final filtered = (activePeer == null || peers.length <= 1)
                ? _history
                : _history.where((item) {
                    final raw = item.peerName.trim().toLowerCase();
                    if (raw.isEmpty || raw == '对端设备') return true;
                    final dName = activePeer.deviceName.trim().toLowerCase();
                    final dId = activePeer.deviceId.trim().toLowerCase();
                    final dIp = (activePeer.ip ?? '').trim().toLowerCase();
                    return raw == dName ||
                        raw == dId ||
                        (dIp.isNotEmpty && raw.contains(dIp));
                  }).toList(growable: false);

            final messages = <_ChatMessage>[
              for (final item in filtered)
                (item: item, status: null, error: null, retry: null),
              for (final pending in _pending.values)
                if (pending.item.peerName == activePeer?.deviceId)
                  (
                    item: pending.item,
                    status: pending.error == null ? '发送中' : '发送失败',
                    error: pending.error,
                    retry: pending.retry,
                  ),
              for (final outgoing in _outbox.values)
                if (outgoing.targets.any(
                    (target) => target.targetDeviceId == activePeer?.deviceId))
                  (
                    item: TransferHistoryModel(
                      id: outgoing.localSeq,
                      direction: 'sent',
                      kind: outgoing.kind,
                      title: outgoing.title,
                      content: outgoing.content,
                      subtitle: '',
                      peerName: activePeer!.deviceId,
                      sizeBytes: outgoing.sizeBytes,
                      createdAtSecs: outgoing.createdAt,
                    ),
                    status: switch (outgoing.overallStatus) {
                      'sent' => '已发送',
                      'failed' => '发送失败',
                      'partial_failed' => '部分发送失败',
                      'unknown' || 'unknown_legacy' => '结果未确认',
                      _ => '发送中',
                    },
                    error: outgoing.targets
                        .where((target) =>
                            target.targetDeviceId == activePeer.deviceId)
                        .first
                        .errorMessage,
                    retry: outgoing.canRetry
                        ? () => _retryOutbox(outgoing.clientMsgId)
                        : null,
                  ),
            ]..sort(
                (a, b) => a.item.createdAtSecs.compareTo(b.item.createdAtSecs));
            final chatPane = _buildChatPane(
              context,
              activePeer: activePeer,
              items: messages,
              isWide: isWide,
            );

            if (!isWide) return chatPane;

            return Row(
              children: <Widget>[
                _buildDesktopPeerSidebar(context, peers, activePeer),
                Expanded(child: chatPane),
              ],
            );
          },
        );
      },
    );
  }

  Widget _buildDesktopPeerSidebar(
    BuildContext context,
    List<PairedPeerModel> peers,
    PairedPeerModel? activePeer,
  ) {
    final neu = NeuTheme.of(context);
    return Container(
      width: 244,
      decoration: BoxDecoration(
        color: neu.chromeSurface,
        border: Border(
          right: BorderSide(color: neu.hairlineBorder, width: 1),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 18, 12, 12),
            child: Row(
              children: <Widget>[
                Text(
                  '设备对话',
                  style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                ),
                const Spacer(),
                IconButton(
                  tooltip: '配对新设备',
                  icon: const Icon(Icons.add_rounded, size: 20),
                  onPressed: widget.onOpenDeviceManager,
                ),
              ],
            ),
          ),
          Expanded(
            child: peers.isEmpty
                ? Padding(
                    padding: const EdgeInsets.all(16),
                    child: NeuEmptyState(
                      icon: Icons.devices_other_rounded,
                      title: '暂无设备',
                      subtitle: '配对后即可像聊天一样互传文件和文字。',
                      action: NeuButton(
                        label: '去配对',
                        tone: NeuButtonTone.primary,
                        onPressed: widget.onOpenDeviceManager,
                      ),
                    ),
                  )
                : ListView.separated(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 4,
                    ),
                    itemCount: peers.length,
                    separatorBuilder: (_, __) => const SizedBox(height: 6),
                    itemBuilder: (context, index) {
                      final peer = peers[index];
                      final selected = peer.deviceId == activePeer?.deviceId;
                      return InkWell(
                        onTap: () => widget.bridge.selectPeer(peer.deviceId),
                        borderRadius:
                            const BorderRadius.all(Radius.circular(12)),
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 12,
                            vertical: 10,
                          ),
                          decoration: BoxDecoration(
                            color: selected
                                ? neu.accentSoft
                                : Colors.transparent,
                            borderRadius:
                                const BorderRadius.all(Radius.circular(12)),
                          ),
                          child: Row(
                            children: <Widget>[
                              NeuIconBadge(
                                icon: peer.osType
                                        .toLowerCase()
                                        .contains('android')
                                    ? Icons.phone_android_rounded
                                    : Icons.desktop_windows_rounded,
                                color: selected
                                    ? neu.accent
                                    : neu.slateMuted,
                                backgroundColor: neu.surface,
                                size: 34,
                                iconSize: 17,
                              ),
                              const SizedBox(width: 10),
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
                                          ?.copyWith(
                                            fontWeight: selected
                                                ? FontWeight.w700
                                                : FontWeight.w500,
                                          ),
                                    ),
                                    const SizedBox(height: 2),
                                    Text(
                                      peer.online ? '在线' : '离线',
                                      style: Theme.of(context)
                                          .textTheme
                                          .labelSmall
                                          ?.copyWith(
                                            color: peer.online
                                                ? neu.healthy
                                                : neu.textSecondary,
                                          ),
                                    ),
                                  ],
                                ),
                              ),
                            ],
                          ),
                        ),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }

  Widget _buildChatPane(
    BuildContext context, {
    required PairedPeerModel? activePeer,
    required List<_ChatMessage> items,
    required bool isWide,
  }) {
    final neu = NeuTheme.of(context);
    return Column(
      children: <Widget>[
        if (isWide && activePeer != null)
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 14),
            decoration: BoxDecoration(
              color: neu.surface,
              border: Border(
                bottom: BorderSide(color: neu.hairlineBorder),
              ),
            ),
            child: Row(
              children: <Widget>[
                Text(
                  activePeer.deviceName,
                  style: Theme.of(context).textTheme.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                ),
                const SizedBox(width: 10),
                NeuStatusBadge(
                  label: activePeer.online ? '在线' : '离线',
                  tone: activePeer.online
                      ? NeuStatusTone.healthy
                      : NeuStatusTone.neutral,
                  showDot: true,
                ),
                const Spacer(),
                IconButton(
                  tooltip: '刷新记录',
                  icon: const Icon(Icons.refresh_rounded, size: 19),
                  onPressed: () => _refreshHistory(silent: false),
                ),
              ],
            ),
          ),
        Expanded(
          child: _loading && items.isEmpty
              ? const Center(child: CircularProgressIndicator())
              : items.isEmpty
                  ? Center(
                      child: Padding(
                        padding: const EdgeInsets.all(24),
                        child: NeuEmptyState(
                          icon: Icons.chat_bubble_outline_rounded,
                          title: activePeer == null
                              ? '先配对一台设备'
                              : '与 ${activePeer.deviceName} 暂无传输记录',
                          subtitle: activePeer == null
                              ? '配对后即可在这里直接互发文件或文字。'
                              : '点击左下角 + 发送文件，或在输入框输入文字直接发送。',
                          action: activePeer == null
                              ? NeuButton(
                                  label: '去配对设备',
                                  tone: NeuButtonTone.primary,
                                  onPressed: widget.onOpenDeviceManager,
                                )
                              : null,
                        ),
                      ),
                    )
                  : ListView.builder(
                      controller: _scrollController,
                      padding: const EdgeInsets.fromLTRB(16, 16, 16, 12),
                      itemCount: items.length,
                      itemBuilder: (context, index) {
                        final message = items[index];
                        final item = message.item;
                        final showTime = index == 0 ||
                            (item.createdAtSecs -
                                        items[index - 1].item.createdAtSecs)
                                    .abs() >
                                180;
                        return _MessageBubbleRow(
                          item: item,
                          showTimeHeader: showTime,
                          status: message.status,
                          error: message.error,
                          onRetry: message.retry,
                        );
                      },
                    ),
        ),
        _buildInputBar(context, enabled: activePeer != null),
      ],
    );
  }

  Widget _buildInputBar(BuildContext context, {required bool enabled}) {
    final neu = NeuTheme.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
      decoration: BoxDecoration(
        color: neu.canvas,
        border: Border(
          top: BorderSide(color: neu.hairlineBorder),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: <Widget>[
          _RoundActionButton(
            icon: Icons.add_rounded,
            tooltip: '发送文件',
            onPressed: enabled ? _pickAndSendFile : null,
          ),
          const SizedBox(width: 8),
          _RoundActionButton(
            icon: Icons.content_paste_rounded,
            tooltip: '粘贴本机剪贴板',
            onPressed: enabled ? _pasteFromClipboard : null,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Container(
              constraints: const BoxConstraints(minHeight: 42, maxHeight: 110),
              padding: const EdgeInsets.symmetric(horizontal: 14),
              decoration: BoxDecoration(
                color: neu.surface,
                borderRadius: const BorderRadius.all(Radius.circular(22)),
                border: Border.all(color: neu.hairlineBorder),
              ),
              child: TextField(
                controller: _inputController,
                enabled: enabled,
                minLines: 1,
                maxLines: 4,
                textInputAction: TextInputAction.send,
                onSubmitted: (_) => unawaited(_sendText()),
                decoration: const InputDecoration(
                  hintText: '输入文字或链接发送…',
                  border: InputBorder.none,
                  isDense: true,
                  contentPadding: EdgeInsets.symmetric(vertical: 11),
                ),
              ),
            ),
          ),
          const SizedBox(width: 8),
          _RoundActionButton(
            icon: Icons.arrow_upward_rounded,
            tooltip: '发送',
            primary: true,
            onPressed: enabled ? _sendText : null,
          ),
        ],
      ),
    );
  }
}

typedef _PendingChatMessage = ({
  TransferHistoryModel item,
  String? error,
  Future<void> Function()? retry,
});

typedef _ChatMessage = ({
  TransferHistoryModel item,
  String? status,
  String? error,
  Future<void> Function()? retry,
});

class _MessageBubbleRow extends StatelessWidget {
  const _MessageBubbleRow({
    required this.item,
    required this.showTimeHeader,
    this.status,
    this.error,
    this.onRetry,
  });

  final TransferHistoryModel item;
  final bool showTimeHeader;
  final String? status;
  final String? error;
  final VoidCallback? onRetry;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final incoming = item.isIncoming;
    final timeStr = item.createdAtSecs > 0
        ? LanVaultBridge.formatRelativeTime('${item.createdAtSecs}')
        : item.formattedTime;

    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Column(
        crossAxisAlignment:
            incoming ? CrossAxisAlignment.start : CrossAxisAlignment.end,
        children: <Widget>[
          if (showTimeHeader && timeStr.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 4, bottom: 10),
              child: Center(
                child: Text(
                  timeStr,
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: neu.textSecondary,
                      ),
                ),
              ),
            ),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 320),
            child: item.isFile
                ? _buildFileBubble(context, incoming)
                : _buildTextBubble(context, incoming),
          ),
          if (status != null)
            Wrap(
              crossAxisAlignment: WrapCrossAlignment.center,
              spacing: 6,
              children: [
                Text(status!,
                    semanticsLabel: error == null ? status : '$status：$error'),
                if (onRetry != null)
                  TextButton(onPressed: onRetry, child: const Text('重试')),
              ],
            ),
        ],
      ),
    );
  }

  Widget _buildFileBubble(BuildContext context, bool incoming) {
    final neu = NeuTheme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: neu.surface,
        borderRadius: const BorderRadius.all(Radius.circular(18)),
        border: Border.all(color: neu.hairlineBorder),
        boxShadow: neu.controlShadows,
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          NeuIconBadge(
            icon: Icons.insert_drive_file_rounded,
            color: neu.accent,
            backgroundColor: neu.accentSoft,
            size: 40,
            iconSize: 20,
          ),
          const SizedBox(width: 12),
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  item.title,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleSmall?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                ),
                const SizedBox(height: 3),
                Text(
                  item.sizeBytes > 0
                      ? SystemEnvHelper.formatBytes(item.sizeBytes)
                      : (incoming ? '已接收文件' : '已发送文件'),
                  style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: neu.textSecondary,
                      ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildTextBubble(BuildContext context, bool incoming) {
    final neu = NeuTheme.of(context);
    final text = (item.content != null && item.content!.isNotEmpty)
        ? item.content!
        : item.title;

    return Semantics(
      button: true,
      onTapHint: '复制消息',
      child: InkWell(
        onTap: () {
          Clipboard.setData(ClipboardData(text: text));
          NeuToast.success(context, '已复制到剪贴板');
        },
        borderRadius: const BorderRadius.all(Radius.circular(18)),
        child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
        decoration: BoxDecoration(
          color: incoming ? neu.surface : neu.accent,
          borderRadius: const BorderRadius.all(Radius.circular(18)),
          border: incoming ? Border.all(color: neu.hairlineBorder) : null,
          boxShadow: neu.controlShadows,
        ),
          child: Text(
            text,
            style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                  color: incoming ? neu.textPrimary : neu.onAccent,
                  height: 1.4,
                ),
          ),
        ),
      ),
    );
  }
}

class _RoundActionButton extends StatelessWidget {
  const _RoundActionButton({
    required this.icon,
    required this.tooltip,
    required this.onPressed,
    this.primary = false,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback? onPressed;
  final bool primary;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final enabled = onPressed != null;
    return Tooltip(
      message: tooltip,
      child: InkWell(
        onTap: onPressed,
        customBorder: const CircleBorder(),
        child: SizedBox(
          width: 48,
          height: 48,
          child: Center(
            child: Container(
              width: 42,
              height: 42,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: primary
                    ? (enabled
                        ? neu.accent
                        : neu.accent.withValues(alpha: 0.35))
                    : neu.surface,
                border:
                    primary ? null : Border.all(color: neu.hairlineBorder),
                boxShadow: neu.controlShadows,
              ),
              child: Icon(
                icon,
                size: 20,
                color: primary
                    ? neu.onAccent
                    : (enabled
                        ? neu.textPrimary
                        : neu.textSecondary.withValues(alpha: 0.4)),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
