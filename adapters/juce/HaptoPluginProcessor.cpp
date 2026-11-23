#include "HaptoPluginProcessor.h"

#include <array>
#include <cstring>
#include <limits>
#include <memory>
#include <utility>

namespace hapto
{

namespace
{

// What a MidiBuffer stores beside each message's bytes: its frame and size.
constexpr std::size_t eventHeader = sizeof (juce::int32) + sizeof (juce::uint16);
// The most MIDI packets one block carries each way, and the longest SysEx
// the plugin may send whole, F0 and F7 included. A MidiBuffer walks its
// messages by a 16-bit size that counts the header too, so a longer one,
// which addEvent still takes, would break the walk.
constexpr std::size_t midiRoom = 2048;
constexpr std::size_t sysexRoom = 65535 - eventHeader;
// The most bytes one block's MIDI out takes in a MidiBuffer: each packet a
// message of its own, at most six bytes and a SysEx's F0 and F7, and one
// SysEx the block ends that began in an earlier block.
constexpr std::size_t midiOutBytes = midiRoom * (eventHeader + 8) + eventHeader + sysexRoom;

// One whole MIDI 1.0 message, as a MidiBuffer holds it, as Universal MIDI
// Packets on the first group, written to `packets` from `count` while they
// have room: a channel message as message type 2, a system message as 1, a
// SysEx as 7-bit SysEx packets of six bytes each.
void addPackets (std::vector<HaptoPacket>& packets, std::uint32_t& count, const juce::uint8* bytes,
                 int size, std::uint32_t frame) noexcept
{
    const auto push = [&] (std::uint32_t first, std::uint32_t second) {
        if (count < packets.size())
            packets[count++] = HaptoPacket { frame, { first, second, 0, 0 } };
    };
    if (size <= 0)
        return;
    const auto status = static_cast<std::uint32_t> (bytes[0]);
    if (status == 0xF0)
    {
        const auto* data = bytes + 1;
        auto length = size - 1;
        if (length > 0 && data[length - 1] == 0xF7)
            --length;
        const auto parts = juce::jmax (1, (length + 5) / 6);
        for (int part = 0; part < parts; ++part)
        {
            const auto chunk = juce::jlimit (0, 6, length - part * 6);
            const std::uint32_t form = parts == 1 ? 0 : part == 0 ? 1 : part == parts - 1 ? 3 : 2;
            std::array<std::uint32_t, 6> b {};
            for (int index = 0; index < chunk; ++index)
                b[static_cast<std::size_t> (index)] = data[part * 6 + index] & 0x7Fu;
            push (0x30000000u | form << 20 | static_cast<std::uint32_t> (chunk) << 16 | b[0] << 8 | b[1],
                  b[2] << 24 | b[3] << 16 | b[4] << 8 | b[5]);
        }
        return;
    }
    const std::uint32_t kind = status >= 0xF0 ? 0x1 : 0x2;
    const std::uint32_t first = size > 1 ? bytes[1] & 0x7Fu : 0;
    const std::uint32_t second = size > 2 ? bytes[2] & 0x7Fu : 0;
    push (kind << 28 | status << 16 | first << 8 | second, 0);
}

// The plugin's table, checked against the header this adapter was built with.
const HaptoPlugin& table() noexcept
{
    return *hapto_plugin();
}

bool isCurrent (const HaptoPlugin& plugin) noexcept
{
    return plugin.abi_version == HAPTO_ABI_VERSION;
}

juce::String textOf (HaptoStr text)
{
    return juce::String::fromUTF8 (text.data, static_cast<int> (text.length));
}

// save's writer: the block replaces what the MemoryBlock held. Called from
// Rust, so it may not throw; an allocation failure ends the process instead.
void writeBlock (void* context, const std::uint8_t* data, std::size_t length) noexcept
{
    static_cast<juce::MemoryBlock*> (context)->replaceAll (data, length);
}

} // namespace

//==============================================================================
// A Hapto parameter, as JUCE sees it: named by its key, with every value read
// from and written to the plugin's store, so there is no second copy to keep
// in step.
class PluginProcessor::Parameter final : public juce::HostedAudioProcessorParameter
{
public:
    Parameter (const HaptoPlugin& plugin, HaptoInstance* instance, std::uint32_t index, const HaptoParamInfo& info)
        : juce::HostedAudioProcessorParameter (1),
          plugin_ (plugin),
          instance_ (instance),
          index_ (index),
          key_ (textOf (info.key)),
          name_ (textOf (info.name)),
          unit_ (textOf (info.unit)),
          kind_ (info.kind),
          steps_ (static_cast<int> (info.steps)),
          default_ (static_cast<float> (info.default_normalized))
    {
        told_.store (getValue());
    }

    // Any thread.
    float getValue() const override { return static_cast<float> (plugin_.get (instance_, index_)); }

    // Any thread, the audio thread included: a host write, which the
    // plugin snaps and stores. The host knows the stored value from then on.
    void setValue (float newValue) override
    {
        plugin_.set (instance_, index_, newValue);
        told_.store (getValue());
    }

    float getDefaultValue() const override { return default_; }
    juce::String getName (int maximumStringLength) const override { return name_.substring (0, maximumStringLength); }
    juce::String getLabel() const override { return unit_; }
    juce::String getParameterID() const override { return key_; }

    // A bool or choice is stepped for the host; a stepped range stays
    // continuous.
    int getNumSteps() const override { return steps_ > 0 ? steps_ : AudioProcessor::getDefaultNumParameterSteps(); }
    bool isDiscrete() const override { return steps_ > 0; }
    bool isBoolean() const override { return kind_ == HAPTO_KIND_BOOL; }

    // Message thread, or any but audio.
    juce::String getText (float normalized, int maximumStringLength) const override
    {
        std::array<char, 256> text {};
        plugin_.text (instance_, index_, normalized, text.data(), text.size());
        return juce::String::fromUTF8 (text.data()).substring (0, maximumStringLength);
    }

    // JUCE has no way to say text means nothing: a failed parse answers the
    // current value, so the host's write changes nothing.
    float getValueForText (const juce::String& text) const override
    {
        // Counted to the NUL: getNumBytesAsUTF8() counts re-encoded characters,
        // which can run past the buffer of a string holding invalid UTF-8.
        const auto* utf8 = text.toRawUTF8();
        double normalized = 0.0;
        if (plugin_.parse (instance_, index_, HaptoStr { utf8, std::strlen (utf8) }, &normalized))
            return static_cast<float> (normalized);
        return getValue();
    }

    // Message thread: tells JUCE, and through it the host, of a value the
    // plugin already stored.
    void tell (float normalized)
    {
        told_.store (normalized);
        sendValueChangedMessageToListeners (normalized);
    }

    // Message thread, after a sync: tells the host the stored value if it
    // is not the last one the host was told or wrote.
    void tellIfMoved()
    {
        const auto value = getValue();
        if (value != told_.load())
            tell (value);
    }

    // Any thread but audio, after a load: the host re-reads every value
    // itself (JUCE's controller does on setComponentState).
    void forgetTold() { told_.store (getValue()); }

private:
    const HaptoPlugin& plugin_;
    HaptoInstance* const instance_;
    const std::uint32_t index_;
    const juce::String key_, name_, unit_;
    const std::uint32_t kind_;
    const int steps_;
    const float default_;
    // What the host was last told or wrote, so a sync tells it only the
    // values that moved. Written on the audio thread too, by setValue.
    std::atomic<float> told_ { 0.0f };
};

//==============================================================================
PluginProcessor::BusesProperties PluginProcessor::busesOf (const HaptoPlugin& plugin)
{
    HaptoLayout layout { 0, 2 };
    if (isCurrent (plugin))
        plugin.layout (0, &layout);

    BusesProperties buses;
    if (layout.inputs > 0)
        buses = buses.withInput ("Input", juce::AudioChannelSet::canonicalChannelSet (static_cast<int> (layout.inputs)), true);
    return buses.withOutput ("Output", juce::AudioChannelSet::canonicalChannelSet (static_cast<int> (layout.outputs)), true);
}

PluginProcessor::PluginProcessor()
    : juce::AudioProcessor (busesOf (table())),
      plugin_ (table())
{
    // A plugin built against another hapto.h would be misread: refuse it.
    jassert (isCurrent (plugin_));
    if (! isCurrent (plugin_))
        return;

    const HaptoHost host { this, &onBeginEdit, &onPerformEdit, &onEndEdit, &onRestart };
    instance_ = plugin_.create (&host);
    if (instance_ == nullptr)
        return;

    const auto count = plugin_.param_count (instance_);
    parameters_.reserve (count);
    for (std::uint32_t index = 0; index < count; ++index)
    {
        // parameters_ is indexed as the plugin's list is, so a gap would
        // misname every later one: stop at the first that cannot be read.
        HaptoParamInfo info {};
        if (! plugin_.param_info (instance_, index, &info))
            break;
        auto parameter = std::make_unique<Parameter> (plugin_, instance_, index, info);
        parameters_.push_back (parameter.get());
        if ((info.flags & HAPTO_PARAM_BYPASS) != 0)
            bypass_ = parameter.get();
        addParameter (parameter.release());
    }

    startTimerHz (60);
}

PluginProcessor::~PluginProcessor()
{
    // The tick first: no idle may run once the instance is gone. stopTimer
    // does not wait for a tick running on another thread, so, as for any
    // JUCE plugin, the processor must be destroyed on the message thread.
    stopTimer();
    if (instance_ != nullptr)
        plugin_.destroy (instance_);
}

void PluginProcessor::prepareToPlay (double sampleRate, int maximumExpectedSamplesPerBlock)
{
    if (instance_ == nullptr)
        return;

    const HaptoLayout layout { static_cast<std::uint32_t> (getMainBusNumInputChannels()),
                              static_cast<std::uint32_t> (getMainBusNumOutputChannels()) };
    const auto frames = static_cast<std::uint32_t> (juce::jmax (1, maximumExpectedSamplesPerBlock));
    // Refused where hapto.h's HaptoPlugin says, or for a layout the plugin does
    // not run with. A plugin that cannot move refuses it when JUCE's AAX
    // wrapper prepares again on the render thread for a longer block. After a
    // refusal, the processor already built plays on, and so does the bypassed
    // path. Its delay line is left alone: blocks may be running.
    const auto activated = plugin_.activate (instance_, sampleRate, frames, layout);
    jassert (activated);
    if (! activated)
        return;
    // The activation's latency, which the host reads when this returns. No
    // block runs meanwhile, so the bypassed path's delay line may be resized.
    reportLatency();
    bypassDelay_.setSize (juce::jmax (1, getTotalNumInputChannels(), getTotalNumOutputChannels()),
                          juce::jmax (1, getLatencySamples()));
    bypassDelay_.clear();
    bypassPosition_ = 0;
    midiIn_.assign (midiRoom, HaptoPacket {});
    midiOut_.assign (midiRoom, HaptoPacket {});
    sysexOut_.clear();
    sysexOut_.reserve (sysexRoom);
    midiOutRoom_.clear();
    midiOutRoom_.ensureSize (midiOutBytes);
    swapOutRoom_ = true;
}

void PluginProcessor::releaseResources()
{
    if (instance_ != nullptr)
        plugin_.deactivate (instance_);
}

void PluginProcessor::reset()
{
    if (instance_ != nullptr)
        plugin_.reset (instance_);
}

bool PluginProcessor::isBusesLayoutSupported (const BusesLayout& layouts) const
{
    if (! isCurrent (plugin_))
        return false;

    const auto inputs = static_cast<std::uint32_t> (layouts.getMainInputChannels());
    const auto outputs = static_cast<std::uint32_t> (layouts.getMainOutputChannels());
    const auto count = plugin_.layout_count();
    for (std::uint32_t index = 0; index < count; ++index)
    {
        HaptoLayout layout {};
        if (plugin_.layout (index, &layout) && layout.inputs == inputs && layout.outputs == outputs)
            return true;
    }
    return false;
}

void PluginProcessor::processBlock (juce::AudioBuffer<float>& buffer, juce::MidiBuffer& midi)
{
    // The buffer carries the block's MIDI in and takes its MIDI out: what came
    // in goes to the plugin as packets, and only what the plugin sends goes
    // back out. Clearing keeps the buffer's storage.
    std::uint32_t received = 0;
    for (const auto message : midi)
        addPackets (midiIn_, received, message.data, message.numBytes,
                    static_cast<std::uint32_t> (juce::jmax (0, message.samplePosition)));
    midi.clear();
    // The first block after prepareToPlay hands the host's buffer the room
    // made there, so writing the plugin's MIDI into it allocates nothing.
    // JUCE's wrappers keep one buffer for an instance, which keeps the room.
    if (std::exchange (swapOutRoom_, false))
        midi.swapWith (midiOutRoom_);
    bypassing_ = false;
    if (instance_ == nullptr)
    {
        buffer.clear();
        return;
    }

    // In place: JUCE's buffer holds the input in its first channels and
    // takes the output in the same ones.
    const auto channels = buffer.getNumChannels();
    std::uint32_t sent = 0;
    const HaptoProcess block { buffer.getArrayOfReadPointers(),
                              buffer.getArrayOfWritePointers(),
                              static_cast<std::uint32_t> (juce::jmin (getMainBusNumInputChannels(), channels)),
                              static_cast<std::uint32_t> (juce::jmin (getMainBusNumOutputChannels(), channels)),
                              static_cast<std::uint32_t> (buffer.getNumSamples()),
                              received,
                              midiIn_.data(),
                              midiOut_.data(),
                              &sent,
                              static_cast<std::uint32_t> (midiOut_.size()),
                              // A MidiBuffer holds MIDI 1.0 alone.
                              HAPTO_MIDI_OUT_1 };
    plugin_.process (instance_, &block);
    for (std::uint32_t index = 0; index < sent && index < midiOut_.size(); ++index)
        addMessage (midi, midiOut_[index]);
}

void PluginProcessor::addMessage (juce::MidiBuffer& midi, const HaptoPacket& packet)
{
    const auto head = packet.words[0];
    const auto frame = static_cast<int> (packet.frame);
    const auto status = static_cast<juce::uint8> ((head >> 16) & 0xFF);
    const std::array<juce::uint8, 3> bytes { status, static_cast<juce::uint8> ((head >> 8) & 0x7F),
                                             static_cast<juce::uint8> (head & 0x7F) };
    switch (head >> 28)
    {
        case 0x2:
        {
            const auto high = status & 0xF0;
            midi.addEvent (bytes.data(), high == 0xC0 || high == 0xD0 ? 2 : 3, frame);
            return;
        }
        case 0x1:
            midi.addEvent (bytes.data(), status == 0xF2 ? 3 : status == 0xF1 || status == 0xF3 ? 2 : 1, frame);
            return;
        case 0x3:
        {
            const auto form = (head >> 20) & 0xF;
            const auto count = juce::jmin (6u, (head >> 16) & 0xF);
            const auto second = packet.words[1];
            const std::array<juce::uint8, 6> data { static_cast<juce::uint8> ((head >> 8) & 0x7F),
                                                    static_cast<juce::uint8> (head & 0x7F),
                                                    static_cast<juce::uint8> ((second >> 24) & 0x7F),
                                                    static_cast<juce::uint8> ((second >> 16) & 0x7F),
                                                    static_cast<juce::uint8> ((second >> 8) & 0x7F),
                                                    static_cast<juce::uint8> (second & 0x7F) };
            // A first or whole packet begins a SysEx; the room was reserved,
            // so joining one allocates nothing, and one past it is dropped.
            if (form == 0 || form == 1)
            {
                sysexOut_.clear();
                sysexOut_.push_back (0xF0);
                sysexOverflow_ = false;
            }
            if (sysexOut_.size() + count + 1 > sysexRoom)
                sysexOverflow_ = true;
            if (! sysexOverflow_)
                sysexOut_.insert (sysexOut_.end(), data.begin(), data.begin() + count);
            if ((form == 0 || form == 3) && ! sysexOut_.empty())
            {
                if (! sysexOverflow_)
                {
                    sysexOut_.push_back (0xF7);
                    midi.addEvent (sysexOut_.data(), static_cast<int> (sysexOut_.size()), frame);
                }
                sysexOut_.clear();
            }
            return;
        }
        default:
            // A MIDI 2.0 or utility packet, which a MidiBuffer cannot hold; the
            // plugin was asked for MIDI 1.0.
            return;
    }
}

void PluginProcessor::processBlockBypassed (juce::AudioBuffer<float>& buffer, juce::MidiBuffer&)
{
    // As JUCE's own does, outputs with no input start silent.
    const auto inputs = juce::jmin (getMainBusNumInputChannels(), buffer.getNumChannels());
    for (auto channel = inputs; channel < buffer.getNumChannels(); ++channel)
        buffer.clear (channel, 0, buffer.getNumSamples());

    // JUCE's own then passes the input through undelayed, which a host that
    // compensates for the latency would play that much early.
    const auto latency = juce::jmin (bypassLatency_.load (std::memory_order_relaxed),
                                     bypassDelay_.getNumSamples());
    if (latency <= 0)
        return;
    // A bypass that starts begins from silence, not from what the last one
    // left in the line; clearing the line allocates nothing.
    if (! bypassing_)
    {
        bypassDelay_.clear();
        bypassPosition_ = 0;
        bypassing_ = true;
    }
    if (bypassPosition_ >= latency)
        bypassPosition_ = 0;

    auto position = bypassPosition_;
    for (auto channel = 0; channel < juce::jmin (inputs, bypassDelay_.getNumChannels()); ++channel)
    {
        auto* samples = buffer.getWritePointer (channel);
        auto* line = bypassDelay_.getWritePointer (channel);
        position = bypassPosition_;
        for (auto index = 0; index < buffer.getNumSamples(); ++index)
        {
            const auto input = samples[index];
            samples[index] = line[position];
            line[position] = input;
            if (++position == latency)
                position = 0;
        }
    }
    bypassPosition_ = position;
}

double PluginProcessor::getTailLengthSeconds() const
{
    if (instance_ == nullptr)
        return 0.0;

    const auto tail = plugin_.tail (instance_);
    if (tail == HAPTO_TAIL_INFINITE)
        return std::numeric_limits<double>::infinity();

    // The rate JUCE's wrapper set at initialisation or setup, as it reads it
    // back to turn these seconds into frames, so a host may ask before the
    // first activation.
    const auto sampleRate = getSampleRate();
    return (tail == HAPTO_TAIL_NONE || sampleRate <= 0.0) ? 0.0 : tail / sampleRate;
}

void PluginProcessor::getStateInformation (juce::MemoryBlock& destination)
{
    if (instance_ != nullptr)
        plugin_.save (instance_, &destination, &writeBlock);
}

void PluginProcessor::setStateInformation (const void* data, int sizeInBytes)
{
    // A block that is not the plugin's changes nothing.
    if (instance_ == nullptr || data == nullptr || sizeInBytes <= 0)
        return;

    if (plugin_.load (instance_, static_cast<const std::uint8_t*> (data), static_cast<std::size_t> (sizeInBytes)))
        for (auto* parameter : parameters_)
            parameter->forgetTold();
}

juce::AudioProcessorParameter* PluginProcessor::getBypassParameter() const
{
    return bypass_;
}

void PluginProcessor::timerCallback()
{
    plugin_.idle (instance_);
}

void PluginProcessor::reportLatency()
{
    const auto latency = static_cast<int> (plugin_.latency (instance_));
    setLatencySamples (latency);
    bypassLatency_.store (latency, std::memory_order_relaxed);
}

PluginProcessor::Parameter* PluginProcessor::parameterAt (std::uint32_t index) const noexcept
{
    return index < parameters_.size() ? parameters_[index] : nullptr;
}

void PluginProcessor::onBeginEdit (void* context, std::uint32_t index) noexcept
{
    if (auto* parameter = static_cast<PluginProcessor*> (context)->parameterAt (index))
        parameter->beginChangeGesture();
}

void PluginProcessor::onPerformEdit (void* context, std::uint32_t index, double normalized) noexcept
{
    // The plugin already stored the value, so this notifies rather than
    // writes, which setValueNotifyingHost would. JUCE's VST3 wrapper sets its
    // own copy and, while the transport is stopped, writes it back through
    // setValue, which stores what the store holds unless a host write
    // landed since.
    if (auto* parameter = static_cast<PluginProcessor*> (context)->parameterAt (index))
        parameter->tell (static_cast<float> (normalized));
}

void PluginProcessor::onEndEdit (void* context, std::uint32_t index) noexcept
{
    if (auto* parameter = static_cast<PluginProcessor*> (context)->parameterAt (index))
        parameter->endChangeGesture();
}

void PluginProcessor::onRestart (void* context, std::uint32_t flags) noexcept
{
    auto& self = *static_cast<PluginProcessor*> (context);
    if ((flags & HAPTO_RESTART_LATENCY) != 0)
        self.reportLatency();

    // JUCE offers no "re-read every value" for a plugin with one program:
    // each parameter that moved tells the host instead, which JUCE's
    // wrappers send on as an edit outside any gesture. A host in latch mode
    // may record those; a sync is rare, and a stale value in the host
    // would be worse. Open: whether Logic and Pro Tools record them through
    // AU is not checked yet.
    if ((flags & HAPTO_RESTART_VALUES) != 0)
        for (auto* parameter : self.parameters_)
            parameter->tellIfMoved();

    // State no parameter holds changed. JUCE's wrappers pass it on in the
    // format's own way, where it has one, so the host offers to save.
    if ((flags & HAPTO_RESTART_STATE) != 0)
        self.updateHostDisplay (ChangeDetails().withNonParameterStateChanged (true));
}

} // namespace hapto

//==============================================================================
juce::AudioProcessor* JUCE_CALLTYPE createPluginFilter()
{
    return new hapto::PluginProcessor();
}
