// Bundled directly into a Blob by Rust. No separate JS build tool or application callback is required.
// The protocol numbers come from Rust's enums through processorOptions.protocol.
class NcPcmProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();
        const p = options.processorOptions;
        this.channels = p.channels;
        this.capacity = p.capacity;
        this.capture = p.capture;
        [this.WRITE, this.RELEASE, this.RESET, this.STATS,
         this.CONSUMED, this.CAPTURE, this.SNAPSHOT, this.RESET_ACK] = p.protocol;
        this.epoch = 0;
        this.first = 0;
        this.count = 0;
        this.credit = this.capacity;
        this.dropped = 0;
        this.silent = 0;
        this.pcm = new Float32Array(this.capacity * this.channels);
        this.port.onmessage = event => this.command(event.data);
    }

    // Whole-frame queue transactions execute between render quanta on this worklet's own thread.
    command(message) {
        if (!Array.isArray(message) || message.length !== 3) throw new Error('invalid PCM command');
        const [op, epoch, value] = message;
        if (op === this.RESET) {
            this.epoch = epoch;
            this.first = this.count = 0;
            this.credit = this.capacity;
            this.port.postMessage([this.RESET_ACK, epoch, null]);
            return;
        }
        if (epoch !== this.epoch) return;
        if (op === this.WRITE && !this.capture) {
            if (!(value instanceof Float32Array) || value.length % this.channels !== 0 ||
                value.length / this.channels > this.capacity - this.count) throw new Error('PCM playback capacity');
            for (let i = 0; i < value.length; i++) {
                this.pcm[((this.first + this.count) * this.channels + i) % this.pcm.length] = value[i];
            }
            this.count += value.length / this.channels;
        } else if (op === this.RELEASE && this.capture) {
            if (!Number.isInteger(value) || value < 0 || this.credit + value > this.capacity) throw new Error('PCM capture credit');
            this.credit += value;
        } else if (op === this.STATS) {
            this.port.postMessage([this.SNAPSHOT, epoch, [this.dropped, this.silent]]);
        } else {
            throw new Error('unknown PCM command');
        }
    }

    process(inputs, outputs) {
        const output = outputs[0];
        const frames = output?.[0]?.length || 0;
        if (!frames) return true;
        // Capture graphs deliberately output silence, preventing microphone feedback.
        if (this.capture) {
            const input = inputs[0];
            const available = input?.[0]?.length || 0;
            const retained = Math.min(available, this.credit);
            this.dropped = Math.min(Number.MAX_SAFE_INTEGER, this.dropped + available - retained);
            if (retained) {
                // Transfers are bounded by credits including messages in flight. Once exhausted,
                // no more capture messages or allocations occur until application reads return credits.
                const data = new Float32Array(retained * this.channels);
                for (let frame = 0; frame < retained; frame++) {
                    for (let channel = 0; channel < this.channels; channel++) {
                        const value = input[channel]?.[frame] ?? input[0]?.[frame] ?? 0;
                        data[frame * this.channels + channel] = Number.isFinite(value) ? Math.max(-1, Math.min(1, value)) : 0;
                    }
                }
                this.credit -= retained;
                this.port.postMessage([this.CAPTURE, this.epoch, [data, this.dropped]], [data.buffer]);
            }
            return true;
        }
        const consumed = Math.min(frames, this.count);
        for (let frame = 0; frame < consumed; frame++) {
            for (let channel = 0; channel < this.channels; channel++) {
                output[channel][frame] = this.pcm[((this.first + frame) * this.channels + channel) % this.pcm.length];
            }
        }
        this.first = (this.first + consumed) % this.capacity;
        this.count -= consumed;
        this.silent = Math.min(Number.MAX_SAFE_INTEGER, this.silent + frames - consumed);
        // Never emit an unbounded stream of empty/starvation messages while the application is idle.
        if (consumed) this.port.postMessage([this.CONSUMED, this.epoch, [consumed, this.silent]]);
        return true;
    }
}
registerProcessor('neocogi-pcm-v1', NcPcmProcessor);
